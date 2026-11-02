//! One pass over a shard of `docs.txt`, answering every query as it goes.

use crate::classify::{CHUNK, classify};
use crate::eval::{Bits, PART, PART_BITS, Postings, Totals, evaluate};
use crate::queries::Compiled;
use crate::terms::Key;

/// Finds where one document's tokens start and stop. `edges` receives the position of each
/// change between token and separator, so tokens are its consecutive pairs. Returns the
/// position of the LF that ends the document, which is what closes a token at the very end.
fn find_token_edges(bytes: &[u8], mut at: usize, edges: &mut Vec<u32>) -> usize {
    edges.clear();
    let mut in_token = 0;
    loop {
        let (mut tokens, line_ends) = match bytes.get(at..at + CHUNK) {
            Some(full) => classify(full.try_into().expect("a full chunk")),
            None => classify_end(bytes, at),
        };
        if line_ends != 0 {
            tokens &= (1 << line_ends.trailing_zeros()) - 1;
        }
        let mut changes = tokens ^ (tokens << 1 | in_token);
        edges.reserve(CHUNK);
        while changes != 0 {
            edges.push(at as u32 + changes.trailing_zeros());
            changes &= changes - 1;
        }
        if line_ends != 0 {
            return at + line_ends.trailing_zeros() as usize;
        }
        in_token = tokens >> (CHUNK - 1);
        at += CHUNK;
    }
}

/// The masks for the last bytes of the file, where a full read would run past the mapping.
#[cold]
fn classify_end(bytes: &[u8], at: usize) -> (u32, u32) {
    let mut padded = [0; CHUNK];
    padded[..bytes.len() - at].copy_from_slice(&bytes[at..]);
    classify(&padded)
}

/// Parses the decimal id at the start of a line. Returns the id and the position of its TAB.
fn parse_id(bytes: &[u8], mut at: usize) -> (u32, usize) {
    let mut id = 0;
    while bytes[at] != b'\t' {
        id = id * 10 + (bytes[at] - b'0') as u32;
        at += 1;
    }
    (id, at)
}

/// What a thread knows about the current part of its shard, until the part is answered.
///
/// A dense term sets a bit, which any number of repeats can do at once. A rare term goes into
/// `log`, which is cheap to write because it is written in order. `index` then sorts the log
/// by term, which gives each rare term a list running from `starts[t]` to `ends[t]` in `lists`.
struct Part {
    dense: Vec<Bits>,
    log: Vec<u32>,
    n_log: usize,
    starts: Vec<u32>,
    ends: Vec<u32>,
    lists: Vec<u16>,
    pending: Vec<u64>,
    result: Box<Bits>,
    scratch: Box<Bits>,
}

impl Part {
    /// `log` and `lists` have room for every token a shard could hold, because a token takes
    /// at least two bytes. Memory that is never written costs nothing, so this is safe to
    /// oversize, and it saves a check on every document.
    fn new(plan: &Compiled, shard_len: usize) -> Self {
        let terms = plan.n_bool_terms as usize;
        Part {
            // One more than there are dense terms: every other token sets that spare entry.
            dense: vec![[0; PART / 64]; plan.n_dense as usize + 1],
            log: vec![0; shard_len / 2 + 1],
            n_log: 0,
            starts: vec![0; terms],
            ends: vec![0; terms],
            lists: vec![0; shard_len / 2 + 1],
            pending: vec![0; plan.n_bool / 64 + 1],
            result: Box::new([0; PART / 64]),
            scratch: Box::new([0; PART / 64]),
        }
    }

    /// Turns tokens into term ids and records them for the document at offset `doc` of the
    /// part. It has no branch on the kind of term, which is close to a coin flip for each token.
    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn add_tokens(
        &mut self,
        plan: &Compiled,
        shard: &[u8],
        tokens: &[[u32; 2]],
        keys: &[(usize, Key)],
        ids: &mut [u32],
        anchored: &mut Vec<usize>,
        doc: usize,
    ) {
        let (word, bit) = (doc / 64, 1u64 << (doc % 64));
        let (n_dense, n_bool_terms) = (plan.n_dense, plan.n_bool_terms);
        let (dense, log) = (&mut self.dense[..], &mut self.log[..]);
        let mut n_log = self.n_log;
        for (position, ((id, &[start, end]), &(home, key))) in
            ids.iter_mut().zip(tokens).zip(keys).enumerate()
        {
            let found = plan
                .table
                .lookup(home, key, shard, start as usize, end as usize);
            let term = found.id();
            *id = term;
            // SAFETY: `dense` has an entry for `n_dense`, `word` is below the length of `Bits`
            // because `doc` is below `PART`, and `log` has room for every token of the shard.
            unsafe {
                *dense
                    .get_unchecked_mut(term.min(n_dense) as usize)
                    .get_unchecked_mut(word) |= bit;
                *log.get_unchecked_mut(n_log) = term << PART_BITS | doc as u32;
            }
            n_log += (term >= n_dense && term < n_bool_terms) as usize;
            if found.is_anchor() {
                anchored.push(position);
            }
        }
        self.n_log = n_log;
    }

    /// Sorts the log into one list for each rare term. The log is in document order, so each
    /// list comes out sorted, and a term repeated in a document is next to itself.
    fn index(&mut self, plan: &Compiled) {
        let log = &self.log[..self.n_log];
        self.starts.fill(0);
        for &entry in log {
            self.starts[(entry >> PART_BITS) as usize] += 1;
        }
        let mut next = 0;
        for start in &mut self.starts[plan.n_dense as usize..] {
            next += std::mem::replace(start, next);
        }
        self.ends.copy_from_slice(&self.starts);
        for &entry in log {
            let term = (entry >> PART_BITS) as usize;
            let doc = (entry & (PART as u32 - 1)) as u16;
            let end = self.ends[term] as usize;
            if end == self.starts[term] as usize || self.lists[end - 1] != doc {
                self.lists[end] = doc;
                self.ends[term] += 1;
            }
        }
    }

    /// Answers the part: every query that always runs, and those a rare term of this part can
    /// trigger, marked in a bitset first so that each runs once, in memory order. Then clears
    /// the part. `part_start` is the shard offset of its first document.
    fn answer(&mut self, plan: &Compiled, totals: &mut Totals, part_start: u32) {
        self.index(plan);
        let n_dense = plan.n_dense as usize;
        for term in n_dense..plan.n_bool_terms as usize {
            if self.starts[term] != self.ends[term] {
                for &query in plan.triggers.get(term as u32) {
                    self.pending[query as usize / 64] |= 1 << (query % 64);
                }
            }
        }
        let (dense, lists, starts, ends) = (&self.dense, &self.lists, &self.starts, &self.ends);
        let postings = |term: u32| {
            let t = term as usize;
            if t < n_dense {
                Postings::Bits(&dense[t])
            } else {
                Postings::List(&lists[starts[t] as usize..ends[t] as usize])
            }
        };
        let (result, scratch) = (&mut *self.result, &mut *self.scratch);
        let mut run = |q: usize| {
            evaluate(&plan.queries[q], postings, result, scratch);
            totals.add_bits(q, result, part_start);
        };
        (0..plan.n_always).for_each(&mut run);
        for (i, pending) in self.pending.iter_mut().enumerate() {
            let mut queries = std::mem::take(pending);
            while queries != 0 {
                run(i * 64 + queries.trailing_zeros() as usize);
                queries &= queries - 1;
            }
        }
        self.dense.iter_mut().for_each(|bits| bits.fill(0));
        self.n_log = 0;
    }
}

/// Finds each token's home in the term table and starts loading it. These loops stand alone
/// so that the compiler has few values to keep track of and can hold them all in registers.
#[inline(never)]
fn hash_tokens(plan: &Compiled, shard: &[u8], tokens: &[[u32; 2]], keys: &mut [(usize, Key)]) {
    for (&[start, end], entry) in tokens.iter().zip(keys) {
        let key = Key::at(shard, start as usize, (end - start) as usize);
        *entry = (plan.table.prefetch(key), key);
    }
}

/// Checks the phrases anchored at position `at` of a document's token ids. The ids are those
/// of this document alone, and separators are not in them, so a phrase cannot cross into
/// another document and any punctuation between its terms is invisible.
#[inline]
fn check_phrases(plan: &Compiled, ids: &[u32], at: usize, doc: u16, totals: &mut Totals) {
    for &(q, anchor_at) in plan.anchors.get(ids[at]) {
        let (q, query) = (q as usize, &plan.queries[q as usize]);
        let Some(start) = at.checked_sub(anchor_at as usize) else {
            continue;
        };
        if totals.last[q] != doc && ids[start..].starts_with(&query.terms[..query.len as usize]) {
            totals.last[q] = doc;
            totals.add_doc(q, doc);
        }
    }
}

/// Scans whole lines of `docs.txt` and returns each query's answer within them.
///
/// `shard` must end just after an LF. Local document numbers stay below `NEVER`, which is
/// checked: at 250,000 documents and at least 12 shards they are around 21,000.
pub fn scan_shard(plan: &Compiled, shard: &[u8]) -> Totals {
    let (base, _) = parse_id(shard, 0);
    let mut totals = Totals::new(base, plan.queries.len());
    let mut part = Part::new(plan, shard.len());
    let (mut edges, mut ids, mut anchored, mut keys) = (vec![], vec![], vec![], vec![]);
    let (mut at, mut doc) = (0, 0u16);
    while at < shard.len() {
        assert!(doc != crate::eval::NEVER, "too many documents in one shard");
        let (_, tab) = parse_id(shard, at);
        let line_end = find_token_edges(shard, tab + 1, &mut edges);
        at = line_end + 1;

        let tokens = edges.as_chunks::<2>().0;
        // These only ever grow, so that a document pays for no clearing.
        if ids.len() < tokens.len() {
            ids.resize(tokens.len(), 0);
            keys.resize(tokens.len(), (0, Key::default()));
        }
        anchored.clear();
        hash_tokens(plan, shard, tokens, &mut keys);
        let offset = doc as usize % PART;
        part.add_tokens(plan, shard, tokens, &keys, &mut ids, &mut anchored, offset);
        for &position in &anchored {
            check_phrases(plan, &ids[..tokens.len()], position, doc, &mut totals);
        }

        if offset == PART - 1 {
            part.answer(plan, &mut totals, doc as u32 + 1 - PART as u32);
        }
        doc = doc.wrapping_add(1);
    }
    if !(doc as usize).is_multiple_of(PART) {
        part.answer(plan, &mut totals, doc as u32 / PART as u32 * PART as u32);
    }
    totals
}
