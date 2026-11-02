//! Bit-parallel BOOL evaluation and the per-shard running totals.

use crate::queries::{Op, Query};

/// How many of the smallest matching ids each query reports.
pub const MAX_IDS: usize = 10;

/// `last` starts here, which no block or document number reaches (see `scan::scan_shard`).
pub const NEVER: u16 = u16::MAX;

/// How many documents a part holds, as a power of two.
pub const PART_BITS: u32 = 11 1024;
pub const PART: usize = 1 << PART_BITS;

/// One bit for each document of a part.
pub type Bits = [u64; PART / 64];

/// 256 bits, the unit `tally` counts in.
type Chunk = [u64; 4];

/// What a part knows about a term: a bit per document, or a rare term's sorted document offsets.
pub enum Postings<'a> {
    Bits(&'a Bits),
    List(&'a [u16]),
}

#[inline]
fn set(bits: &mut Bits, doc: u16) {
    bits[doc as usize / 64] |= 1 << (doc % 64);
}

/// Applies one step of a query to the running result `r`. `scratch` is all zero on entry and
/// exit. It lets `AND` with a list stay as cheap as the list is short.
#[inline]
fn apply(r: &mut Bits, scratch: &mut Bits, term: Postings, op: Op) {
    match (term, op) {
        (Postings::Bits(w), Op::And) => r.iter_mut().zip(w).for_each(|(r, w)| *r &= w),
        (Postings::Bits(w), Op::Or) => r.iter_mut().zip(w).for_each(|(r, w)| *r |= w),
        (Postings::Bits(w), Op::Not) => r.iter_mut().zip(w).for_each(|(r, w)| *r &= !w),
        (Postings::List(list), Op::Or) => list.iter().for_each(|&doc| set(r, doc)),
        (Postings::List(list), Op::Not) => {
            for &doc in list {
                r[doc as usize / 64] &= !(1 << (doc % 64));
            }
        }
        (Postings::List(list), Op::And) => {
            list.iter().for_each(|&doc| set(scratch, doc));
            r.iter_mut().zip(scratch.iter()).for_each(|(r, w)| *r &= w);
            list.iter().for_each(|&doc| scratch[doc as usize / 64] = 0);
        }
    }
}

/// Evaluates a BOOL query on a part, strictly left to right as the spec defines it. Each bit
/// stands for one document, so the three steps are the spec's three set operations. There is
/// no "all documents" set, so `r` can only ever hold documents that contain a query term.
#[inline]
pub fn evaluate<'a>(
    query: &Query,
    postings: impl Fn(u32) -> Postings<'a>,
    r: &mut Bits,
    scratch: &mut Bits,
) {
    r.fill(0);
    apply(r, scratch, postings(query.terms[0]), Op::Or);
    for i in 1..query.len as usize {
        apply(r, scratch, postings(query.terms[i]), query.ops[i - 1]);
    }
}

/// The sum of the positions of the set bits. Each mask picks the positions with one binary
/// digit set, so this adds up that digit's value over all the set bits at once.
#[cfg(not(target_feature = "avx2"))]
#[inline]
fn sum_of_bit_positions(word: u64) -> u32 {
    const MASKS: [u64; 6] = [
        0xAAAA_AAAA_AAAA_AAAA,
        0xCCCC_CCCC_CCCC_CCCC,
        0xF0F0_F0F0_F0F0_F0F0,
        0xFF00_FF00_FF00_FF00,
        0xFFFF_0000_FFFF_0000,
        0xFFFF_FFFF_0000_0000,
    ];
    MASKS
        .iter()
        .enumerate()
        .map(|(digit, mask)| (word & mask).count_ones() << digit)
        .sum()
}

/// How many bits are set, and the sum of their positions.
#[cfg(not(target_feature = "avx2"))]
#[inline]
fn tally(word: &Chunk) -> (u32, u32) {
    let (mut count, mut sum) = (0, 0);
    for (i, &part) in word.iter().enumerate() {
        count += part.count_ones();
        sum += part.count_ones() * 64 * i as u32 + sum_of_bit_positions(part);
    }
    (count, sum)
}

/// How many bits are set, and the sum of their positions, using lookup tables on each half of
/// each byte.
#[cfg(target_feature = "avx2")]
#[inline]
fn tally(word: &Chunk) -> (u32, u32) {
    use std::arch::x86_64::*;
    // SAFETY: the cfg above means this build only runs on CPUs with AVX2, and the load is an
    // unaligned read of the 32 bytes of `word`.
    unsafe {
        let bits = _mm256_broadcastsi128_si256(_mm_setr_epi8(
            0, 1, 1, 2, 1, 2, 2, 3, 1, 2, 2, 3, 2, 3, 3, 4,
        ));
        let low_sum = _mm256_broadcastsi128_si256(_mm_setr_epi8(
            0, 0, 1, 1, 2, 2, 3, 3, 3, 3, 4, 4, 5, 5, 6, 6,
        ));
        let high_sum = _mm256_broadcastsi128_si256(_mm_setr_epi8(
            0, 4, 5, 9, 6, 10, 11, 15, 7, 11, 12, 16, 13, 17, 18, 22,
        ));
        let index = _mm256_setr_epi8(
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
            24, 25, 26, 27, 28, 29, 30, 31,
        );
        let bytes = _mm256_loadu_si256(word.as_ptr().cast());
        let nibble = _mm256_set1_epi8(0x0F);
        let low = _mm256_and_si256(bytes, nibble);
        let high = _mm256_and_si256(_mm256_srli_epi16::<4>(bytes), nibble);
        // Per byte: how many bits are set, and the sum of their positions within the byte.
        let count = _mm256_add_epi8(
            _mm256_shuffle_epi8(bits, low),
            _mm256_shuffle_epi8(bits, high),
        );
        let inner = _mm256_add_epi8(
            _mm256_shuffle_epi8(low_sum, low),
            _mm256_shuffle_epi8(high_sum, high),
        );
        // A bit of byte `b` also stands 8 * b positions along.
        let weighted = _mm256_madd_epi16(_mm256_maddubs_epi16(count, index), _mm256_set1_epi16(1));
        let weighted = _mm256_add_epi64(
            _mm256_and_si256(weighted, _mm256_set1_epi64x(0xFFFF_FFFF)),
            _mm256_srli_epi64::<32>(weighted),
        );
        let zero = _mm256_setzero_si256();
        let sum = _mm256_add_epi64(
            _mm256_sad_epu8(inner, zero),
            _mm256_slli_epi64::<3>(weighted),
        );
        // The count in the top half of each lane and the sum below, then all four lanes added.
        let packed = _mm256_add_epi64(sum, _mm256_slli_epi64::<32>(_mm256_sad_epu8(count, zero)));
        let half = _mm_add_epi64(
            _mm256_castsi256_si128(packed),
            _mm256_extracti128_si256::<1>(packed),
        );
        let total = _mm_cvtsi128_si64(_mm_add_epi64(half, _mm_unpackhi_epi64(half, half))) as u64;
        ((total >> 32) as u32, total as u32)
    }
}

/// Running answers for every query, for one shard of the corpus. Document numbers here are
/// local to the shard, which keeps them in 16 bits. The fields are separate arrays so that
/// the ones used on every evaluation stay close together in memory.
pub struct Totals {
    /// The id of the shard's first document.
    pub base: u32,
    /// For a phrase, the last document counted.
    pub last: Vec<u16>,
    pub count: Vec<u16>,
    pub sum: Vec<u32>,
    pub first: Vec<[u16; MAX_IDS]>,
}

impl Totals {
    pub fn new(base: u32, n_queries: usize) -> Self {
        Totals {
            base,
            last: vec![NEVER; n_queries],
            count: vec![0; n_queries],
            sum: vec![0; n_queries],
            first: vec![[0; MAX_IDS]; n_queries],
        }
    }

    /// Records the matches in `bits` for the part starting at document `part_start`.
    #[inline]
    pub fn add_bits(&mut self, query: usize, bits: &Bits, part_start: u32) {
        let (mut matches, mut positions) = (0, 0);
        for (i, chunk) in bits.as_chunks::<4>().0.iter().enumerate() {
            let (count, sum) = tally(chunk);
            matches += count;
            positions += sum + count * 256 * i as u32;
        }
        let seen = self.count[query] as usize;
        if matches > 0 && seen < MAX_IDS {
            self.add_first(query, seen, bits, part_start);
        }
        self.count[query] += matches as u16;
        self.sum[query] += matches * part_start + positions;
    }

    /// Fills a query's list of smallest ids.
    fn add_first(&mut self, query: usize, seen: usize, bits: &Bits, part_start: u32) {
        let mut slots = self.first[query][seen..].iter_mut();
        for (i, &word) in bits.iter().enumerate() {
            let mut rest = word;
            while rest != 0 {
                let Some(slot) = slots.next() else { return };
                *slot = (part_start + i as u32 * 64 + rest.trailing_zeros()) as u16;
                rest &= rest - 1;
            }
        }
    }

    /// Records one matching document.
    #[inline]
    pub fn add_doc(&mut self, query: usize, doc: u16) {
        let seen = self.count[query] as usize;
        if seen < MAX_IDS {
            self.first[query][seen] = doc;
        }
        self.count[query] += 1;
        self.sum[query] += doc as u32;
    }
}
