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
    // SAFETY: a query's terms are all ids of BOOL terms, which index `words`.
    let word = |term: u32| unsafe { *words.get_unchecked(term as usize) };
    let mut r = word(query.terms[0]);
    for i in 1..query.len as usize {
        let w = word(query.terms[i]);
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

/// One query's running answer within a shard, small enough that a sweep over queries reads
/// few cache lines.
#[derive(Clone, Copy)]
pub struct Tally {
    pub sum: u32,
    pub count: u16,
    /// For a phrase, the last document counted.
    pub last: u16,
}

/// Running answers for every query, for one shard of the corpus. Document numbers here are
/// local to the shard, which keeps them in 16 bits.
pub struct Totals {
    /// The id of the shard's first document.
    pub base: u32,
    pub tally: Vec<Tally>,
    /// The first ids are read and written far less often, so they live apart.
    pub first: Vec<[u16; MAX_IDS]>,
}

impl Totals {
    pub fn new(base: u32, n_queries: usize) -> Self {
        Totals {
            base,
            tally: vec![
                Tally {
                    sum: 0,
                    count: 0,
                    last: NEVER
                };
                n_queries
            ],
            first: vec![[0; MAX_IDS]; n_queries],
        }
    }

    /// Records the matches in `word` for the block starting at document `block_start`.
    #[inline]
    pub fn add_word(&mut self, query: usize, word: u64, block_start: u32) {
        let matches = word.count_ones();
        let tally = &mut self.tally[query];
        let seen = tally.count as usize;
        let mut rest = word;
        for slot in self.first[query].iter_mut().skip(seen) {
            if rest == 0 {
                break;
            }
            *slot = (block_start + rest.trailing_zeros()) as u16;
            rest &= rest - 1;
        }
        tally.count += matches as u16;
        tally.sum += matches * block_start + sum_of_bit_positions(word);
    }

    /// Records one matching document.
    #[inline]
    pub fn add_doc(&mut self, query: usize, doc: u16) {
        let tally = &mut self.tally[query];
        let seen = tally.count as usize;
        if seen < MAX_IDS {
            self.first[query][seen] = doc;
        }
        tally.count += 1;
        tally.sum += doc as u32;
    }
}
