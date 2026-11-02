//! Bit-parallel BOOL evaluation and the per-shard running totals.

use crate::queries::{Form, Op, Query};

/// How many of the smallest matching ids each query reports.
pub const MAX_IDS: usize = 10;

/// `last` starts here, which no block or document number reaches (see `scan::scan_shard`).
pub const NEVER: u16 = u16::MAX;

/// Starts loading the cache line that holds `value`.
#[inline]
pub fn prefetch<T>(value: &T) {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: prefetching has no effect other than on the cache.
    unsafe {
        use std::arch::x86_64::{_MM_HINT_T0, _mm_prefetch};
        _mm_prefetch::<_MM_HINT_T0>(std::ptr::from_ref(value).cast());
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = value;
}

/// One bit for each document of a block.
pub type Word = [u64; 4];

/// How many documents a block holds.
pub const BLOCK: usize = 256;

#[inline]
fn combine(r: Word, w: Word, op: Op) -> Word {
    std::array::from_fn(|i| match op {
        Op::And => r[i] & w[i],
        Op::Or => r[i] | w[i],
        Op::Not => r[i] & !w[i],
    })
}

/// Evaluates a BOOL query on a block, strictly left to right as the spec defines it. Each bit
/// stands for one document, so `&`, `|` and `& !` are the spec's three set operations. There is
/// no "all documents" set, so `r` can only ever hold documents that contain a query term.
/// `slots[t]` says which entry of `words` holds term `t`.
#[inline]
pub fn evaluate(query: &Query, words: &[u64], stride: usize, slots: &[u32]) -> Word {
    let mut r = read(words, stride, slots[query.terms[0] as usize] as usize);
    for i in 1..query.len as usize {
        let w = read(words, stride, slots[query.terms[i] as usize] as usize);
        r = combine(r, w, query.ops[i - 1]);
    }
    r
}

/// The four parts of entry `slot`, which `words` keeps in four separate runs.
#[inline]
fn read(words: &[u64], stride: usize, slot: usize) -> Word {
    let parts = &words[slot..slot + 3 * stride + 1];
    [parts[0], parts[stride], parts[2 * stride], parts[3 * stride]]
}

/// Evaluates a query's folded form, whose terms are all dense, so they index `words` directly.
#[inline]
pub fn evaluate_form(form: &Form, words: &[u64], stride: usize) -> Word {
    let mut r = read(words, stride, form.terms[0] as usize);
    for i in 1..form.len as usize {
        r = combine(r, read(words, stride, form.terms[i] as usize), form.ops[i - 1]);
    }
    r
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
fn tally(word: &Word) -> (u32, u32) {
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
fn tally(word: &Word) -> (u32, u32) {
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
        let count = _mm256_add_epi8(_mm256_shuffle_epi8(bits, low), _mm256_shuffle_epi8(bits, high));
        let inner = _mm256_add_epi8(
            _mm256_shuffle_epi8(low_sum, low),
            _mm256_shuffle_epi8(high_sum, high),
        );
        // A bit of byte `b` also stands 8 * b positions along.
        let weighted = _mm256_madd_epi16(
            _mm256_maddubs_epi16(count, index),
            _mm256_set1_epi16(1),
        );
        let weighted = _mm256_add_epi64(
            _mm256_and_si256(weighted, _mm256_set1_epi64x(0xFFFF_FFFF)),
            _mm256_srli_epi64::<32>(weighted),
        );
        let zero = _mm256_setzero_si256();
        let sum = _mm256_add_epi64(_mm256_sad_epu8(inner, zero), _mm256_slli_epi64::<3>(weighted));
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

    /// Starts loading what recording a match of `query` will touch.
    #[inline]
    pub fn prefetch(&self, query: usize) {
        prefetch(&self.count[query]);
        prefetch(&self.sum[query]);
    }

    /// Records the matches in `word` for the block starting at document `block_start`.
    #[inline]
    pub fn add_block(&mut self, query: usize, word: &Word, block_start: u32) {
        let (matches, positions) = tally(word);
        let seen = self.count[query] as usize;
        if seen < MAX_IDS {
            self.add_first(query, seen, word, block_start);
        }
        self.count[query] += matches as u16;
        self.sum[query] += matches * block_start + positions;
    }

    /// Fills a query's list of smallest ids.
    fn add_first(&mut self, query: usize, seen: usize, word: &Word, block_start: u32) {
        let mut slots = self.first[query][seen..].iter_mut();
        for (i, &part) in word.iter().enumerate() {
            let mut rest = part;
            while rest != 0 {
                let Some(slot) = slots.next() else { return };
                *slot = (block_start + i as u32 * 64 + rest.trailing_zeros()) as u16;
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
