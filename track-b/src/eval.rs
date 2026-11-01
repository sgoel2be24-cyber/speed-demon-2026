//! Bit-parallel BOOL evaluation and the per-shard running totals.

use crate::queries::{Op, Query};

/// How many of the smallest matching ids each query reports.
pub const MAX_IDS: usize = 10;

/// `last` starts here, which no block or document number reaches (see `scan::scan_shard`).
pub const NEVER: u16 = u16::MAX;

/// Evaluates a BOOL query on 64 documents at once, strictly left to right as the spec
/// defines it. Each bit stands for one document, so `&`, `|` and `& !` are the spec's three
/// set operations. There is no "all documents" set, so `r` can only ever hold documents
/// that contain one of the query's terms.
#[inline]
pub fn evaluate(query: &Query, words: &[u64]) -> u64 {
    let mut r = words[query.terms[0] as usize];
    for i in 1..query.len {
        let w = words[query.terms[i] as usize];
        r = match query.ops[i - 1] {
            Op::And => r & w,
            Op::Or => r | w,
            Op::Not => r & !w,
        };
    }
    r
}

/// The sum of the positions of the set bits. Each mask picks the positions with one binary
/// digit set, so this adds up that digit's value over all the set bits at once.
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

    /// Records the matches in `word` for the block starting at document `block_start`.
    #[inline]
    pub fn add_word(&mut self, query: usize, word: u64, block_start: u32) {
        let matches = word.count_ones();
        let seen = self.count[query] as usize;
        let mut rest = word;
        for slot in self.first[query].iter_mut().skip(seen) {
            if rest == 0 {
                break;
            }
            *slot = (block_start + rest.trailing_zeros()) as u16;
            rest &= rest - 1;
        }
        self.count[query] += matches as u16;
        self.sum[query] += matches * block_start + sum_of_bit_positions(word);
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
