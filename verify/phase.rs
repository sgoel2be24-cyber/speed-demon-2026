//! Experimental shared-phase bitmaps and sparse rare-term postings.
use crate::eval::{MAX_IDS, Totals};
use crate::queries::{Compiled, Op, Query};
use crate::terms::Key;
use std::borrow::Cow;
use std::thread;

const PHASE: usize = 16384;

struct Words {
    pointer: *mut u64,
    stride: usize,
}

// Workers own disjoint, whole 64-document words. They never write the same u64,
// even for the same term. The parent reads only after all workers have joined.
unsafe impl Sync for Words {}

impl Words {
    fn insert(&self, term: u32, doc: usize) {
        // SAFETY: term is dense, doc is inside the phase, and the worker's range
        // starts/ends on 64-document boundaries. The allocation outlives all workers.
        unsafe {
            let word = self.pointer.add(term as usize * self.stride + doc / 64);
            *word |= 1 << (doc % 64);
        }
    }
}

struct Answers {
    count: Vec<u32>,
    sum: Vec<u64>,
    first: Vec<[u32; MAX_IDS]>,
}

impl Answers {
    fn new(count: usize) -> Self {
        Self {
            count: vec![0; count],
            sum: vec![0; count],
            first: vec![[0; MAX_IDS]; count],
        }
    }

    fn merge(&mut self, q: usize, count: u32, sum: u64, first: &[u32]) {
        let seen = (self.count[q] as usize).min(MAX_IDS);
        let take = first.len().min(MAX_IDS - seen).min(count as usize);
        self.first[q][seen..seen + take].copy_from_slice(&first[..take]);
        self.count[q] += count;
        self.sum[q] += sum;
    }
}

fn line_offsets(bytes: &[u8]) -> Vec<u32> {
    let mut offsets = vec![0];
    for (chunk, full) in bytes.chunks_exact(32).enumerate() {
        let mut ends = newline_mask(full.try_into().unwrap());
        while ends != 0 {
            offsets.push((chunk * 32 + ends.trailing_zeros() as usize + 1) as u32);
            ends &= ends - 1;
        }
    }
    let start = bytes.len() / 32 * 32;
    for (i, &byte) in bytes[start..].iter().enumerate() {
        if byte == b'\n' {
            offsets.push((start + i + 1) as u32);
        }
    }
    offsets
}

#[cfg(target_feature = "avx2")]
fn newline_mask(bytes: &[u8; 32]) -> u32 {
    use std::arch::x86_64::*;
    // SAFETY: this build has AVX2 and the unaligned load is exactly 32 valid bytes.
    unsafe {
        _mm256_movemask_epi8(_mm256_cmpeq_epi8(
            _mm256_loadu_si256(bytes.as_ptr().cast()),
            _mm256_set1_epi8(b'\n' as i8),
        )) as u32
    }
}

#[cfg(not(target_feature = "avx2"))]
fn newline_mask(bytes: &[u8; 32]) -> u32 {
    bytes
        .iter()
        .enumerate()
        .fold(0, |mask, (i, &b)| mask | (u32::from(b == b'\n') << i))
}

fn scan(
    plan: &Compiled,
    docs: &[u8],
    offsets: &[u32],
    phase: usize,
    begin: usize,
    end: usize,
    words: &Words,
) -> (Vec<(u32, u16)>, Totals) {
    let mut postings = Vec::new();
    let mut last_rare = vec![u16::MAX; (plan.n_bool_terms - plan.n_dense) as usize];
    let mut phrases = Totals::new(begin as u32, plan.queries.len() - plan.n_bool);
    let (mut edges, mut keys, mut ids, mut anchored) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for doc in begin..end {
        let (_, tab) = crate::scan::parse_id(docs, offsets[doc] as usize);
        crate::scan::find_token_edges(docs, tab + 1, &mut edges);
        let tokens = edges.as_chunks::<2>().0;
        if ids.len() < tokens.len() {
            keys.resize(tokens.len(), (0, Key::default()));
            ids.resize(tokens.len(), 0);
        }
        anchored.clear();
        crate::scan::hash_tokens(plan, docs, tokens, &mut keys);
        for (at, ((id, &[start, end]), &(home, key))) in
            ids.iter_mut().zip(tokens).zip(&keys).enumerate()
        {
            let found = plan
                .table
                .lookup(home, key, docs, start as usize, end as usize);
            let term = found.id();
            *id = term;
            if found.is_anchor() {
                anchored.push(at);
            }
            if term < plan.n_dense {
                words.insert(term, doc - phase);
            } else if term < plan.n_bool_terms {
                let rare = term - plan.n_dense;
                if last_rare[rare as usize] != (doc - phase) as u16 {
                    last_rare[rare as usize] = (doc - phase) as u16;
                    postings.push((rare, (doc - phase) as u16));
                }
            }
        }
        let local = (doc - begin) as u16;
        for &at in &anchored {
            for &(q, anchor) in plan.anchors.get(ids[at]) {
                let q = q as usize;
                let target = q - plan.n_bool;
                let Some(start) = at.checked_sub(anchor as usize) else {
                    continue;
                };
                let query = &plan.queries[q];
                if phrases.last[target] != local
                    && ids[start..tokens.len()].starts_with(&query.terms[..query.len as usize])
                {
                    phrases.last[target] = local;
                    phrases.add_doc(target, local);
                }
            }
        }
    }
    (postings, phrases)
}

fn term_present(
    term: u32,
    doc: u16,
    plan: &Compiled,
    dense: &[u64],
    stride: usize,
    offsets: &[usize],
    postings: &[u16],
) -> bool {
    if term < plan.n_dense {
        dense[term as usize * stride + doc as usize / 64] >> (doc % 64) & 1 != 0
    } else {
        let term = (term - plan.n_dense) as usize;
        postings[offsets[term]..offsets[term + 1]]
            .binary_search(&doc)
            .is_ok()
    }
}

fn matches(
    query: &Query,
    doc: u16,
    plan: &Compiled,
    dense: &[u64],
    stride: usize,
    offsets: &[usize],
    postings: &[u16],
) -> bool {
    let mut result = term_present(query.terms[0], doc, plan, dense, stride, offsets, postings);
    for i in 1..query.len as usize {
        let present = term_present(query.terms[i], doc, plan, dense, stride, offsets, postings);
        result = match query.ops[i - 1] {
            Op::And => result && present,
            Op::Or => result || present,
            Op::Not => result && !present,
        };
    }
    result
}

fn stats(bits: &[u64], base: u32) -> (u32, u64, Vec<u32>) {
    let mut count = 0u32;
    let mut sum = 0u64;
    let mut first = Vec::with_capacity(MAX_IDS);
    for (i, part) in bits.chunks(4).enumerate() {
        let mut word = [0; 4];
        word[..part.len()].copy_from_slice(part);
        if word == [0; 4] {
            continue;
        }
        let (n, positions) = crate::eval::tally(&word);
        count += n;
        sum += positions as u64 + n as u64 * (base as u64 + i as u64 * 256);
        if first.len() < MAX_IDS {
            for (j, &value) in part.iter().enumerate() {
                let mut rest = value;
                while rest != 0 && first.len() < MAX_IDS {
                    first.push(base + i as u32 * 256 + j as u32 * 64 + rest.trailing_zeros());
                    rest &= rest - 1;
                }
            }
        }
    }
    (count, sum, first)
}

fn evaluate(
    plan: &Compiled,
    dense: &[u64],
    stride: usize,
    offsets: &[usize],
    postings: &[u16],
    begin: usize,
    end: usize,
    base: u32,
) -> Answers {
    let mut answers = Answers::new(end - begin);
    let mut candidates = Vec::new();
    for q in begin..end {
        let query = &plan.queries[q];
        candidates.clear();
        for &term in &query.terms[..query.len as usize] {
            if term >= plan.n_dense {
                let rare = (term - plan.n_dense) as usize;
                candidates.extend_from_slice(&postings[offsets[rare]..offsets[rare + 1]]);
            }
        }
        candidates.sort_unstable();
        candidates.dedup();
        if let Some(form) = plan.forms.get(q) {
            let start = form.terms[0] as usize * stride;
            let mut bits: Cow<'_, [u64]> = Cow::Borrowed(&dense[start..start + stride]);
            for i in 1..form.len as usize {
                let start = form.terms[i] as usize * stride;
                let term = &dense[start..start + stride];
                match form.ops[i - 1] {
                    Op::And => bits
                        .to_mut()
                        .iter_mut()
                        .zip(term)
                        .for_each(|(r, &w)| *r &= w),
                    Op::Or => bits
                        .to_mut()
                        .iter_mut()
                        .zip(term)
                        .for_each(|(r, &w)| *r |= w),
                    Op::Not => bits
                        .to_mut()
                        .iter_mut()
                        .zip(term)
                        .for_each(|(r, &w)| *r &= !w),
                }
            }
            for &doc in &candidates {
                let matched = matches(query, doc, plan, dense, stride, offsets, postings);
                let word = &mut bits.to_mut()[doc as usize / 64];
                let mask = 1 << (doc % 64);
                *word = (*word & !mask) | (u64::from(matched) << (doc % 64));
            }
            let (count, sum, first) = stats(&bits, base);
            answers.merge(q - begin, count, sum, &first);
        } else {
            let mut count = 0;
            let mut sum = 0;
            let mut first = Vec::with_capacity(MAX_IDS);
            for &doc in &candidates {
                if matches(query, doc, plan, dense, stride, offsets, postings) {
                    count += 1;
                    sum += base as u64 + doc as u64;
                    if first.len() < MAX_IDS {
                        first.push(base + doc as u32);
                    }
                }
            }
            answers.merge(q - begin, count, sum, &first);
        }
    }
    answers
}

pub fn run(plan: &Compiled, docs: &[u8], threads: usize) -> Vec<u8> {
    let offsets = line_offsets(docs);
    let documents = offsets.len() - 1;
    let mut answers = Answers::new(plan.queries.len());
    for phase in (0..documents).step_by(PHASE) {
        let count = (documents - phase).min(PHASE);
        let stride = count.div_ceil(64);
        let workers = threads.max(1).min(stride);
        let mut dense = vec![0u64; plan.n_dense as usize * stride];
        let words = Words {
            pointer: dense.as_mut_ptr(),
            stride,
        };
        let scanned: Vec<_> = thread::scope(|scope| {
            let offsets = &offsets;
            let words = &words;
            (0..workers)
                .map(|worker| {
                    let begin = phase + (stride * worker / workers * 64).min(count);
                    let end = phase + (stride * (worker + 1) / workers * 64).min(count);
                    scope.spawn(move || scan(plan, docs, offsets, phase, begin, end, words))
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .collect()
        });
        let mut rare_offsets = vec![0usize; (plan.n_bool_terms - plan.n_dense) as usize + 1];
        for (postings, phrases) in &scanned {
            for &(term, _) in postings {
                rare_offsets[term as usize + 1] += 1;
            }
            for q in 0..phrases.count.len() {
                let first: Vec<u32> = phrases.first[q]
                    .iter()
                    .take((phrases.count[q] as usize).min(MAX_IDS))
                    .map(|&id| phrases.base + id as u32)
                    .collect();
                let count = phrases.count[q] as u32;
                answers.merge(
                    plan.n_bool + q,
                    count,
                    phrases.sum[q] as u64 + count as u64 * phrases.base as u64,
                    &first,
                );
            }
        }
        for i in 1..rare_offsets.len() {
            rare_offsets[i] += rare_offsets[i - 1];
        }
        let mut next = rare_offsets.clone();
        let mut postings = vec![0u16; *rare_offsets.last().unwrap()];
        // Workers are joined in document order and each records in document order;
        // scattering preserves the sorted order of each term's postings.
        for (records, _) in scanned {
            for (term, doc) in records {
                postings[next[term as usize]] = doc;
                next[term as usize] += 1;
            }
        }
        let batches: Vec<_> = thread::scope(|scope| {
            let dense = &dense;
            let rare_offsets = &rare_offsets;
            let postings = &postings;
            (0..threads.max(1))
                .map(|worker| {
                    let begin = plan.n_bool * worker / threads.max(1);
                    let end = plan.n_bool * (worker + 1) / threads.max(1);
                    scope.spawn(move || {
                        (
                            begin,
                            evaluate(
                                plan,
                                dense,
                                stride,
                                rare_offsets,
                                postings,
                                begin,
                                end,
                                phase as u32,
                            ),
                        )
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .collect()
        });
        for (begin, batch) in batches {
            for q in 0..batch.count.len() {
                answers.merge(
                    begin + q,
                    batch.count[q],
                    batch.sum[q],
                    &batch.first[q][..(batch.count[q] as usize).min(MAX_IDS)],
                );
            }
        }
    }
    let mut output = Vec::new();
    for &q in &plan.position_of_line {
        let q = q as usize;
        crate::output::push_number(&mut output, answers.count[q] as u64);
        output.push(b' ');
        crate::output::push_number(&mut output, answers.sum[q]);
        for &id in &answers.first[q][..(answers.count[q] as usize).min(MAX_IDS)] {
            output.push(b' ');
            crate::output::push_number(&mut output, id as u64);
        }
        output.push(b'\n');
    }
    output
}
