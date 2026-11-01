//! One pass over a shard of `docs.txt`, answering every query as it goes.

use crate::eval::{Totals, evaluate};
use crate::queries::Compiled;
use crate::terms::NONE;

/// Maps a byte to its lowercase token form, or to 0 if it is a separator. Only ASCII letters
/// and digits are token bytes: every byte of 0x80 or above separates, even inside a letter.
const TOKEN_BYTE: [u8; 256] = {
    let mut table = [0u8; 256];
    let mut b = 0;
    while b < 256 {
        table[b] = match b as u8 {
            c @ (b'a'..=b'z' | b'0'..=b'9') => c,
            c @ b'A'..=b'Z' => c + 32,
            _ => 0,
        };
        b += 1;
    }
    table
};

/// Splits one document's text into tokens. `lower` receives the lowercase bytes of all the
/// tokens, end to end, and `ends` the offset in `lower` where each token stops. Returns the
/// position of the LF that ends the document.
fn tokenize(bytes: &[u8], mut at: usize, lower: &mut Vec<u8>, ends: &mut Vec<u32>) -> usize {
    lower.clear();
    ends.clear();
    let mut closed = 0;
    loop {
        let byte = bytes[at];
        let token_byte = TOKEN_BYTE[byte as usize];
        if token_byte != 0 {
            lower.push(token_byte);
        } else if lower.len() > closed {
            closed = lower.len();
            ends.push(closed as u32);
        }
        if byte == b'\n' {
            return at;
        }
        at += 1;
    }
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

/// The terms of the current block of 64 documents. `words[t]` has bit `i` set if document
/// `i` of the block contains term `t`. `touched` lists the terms whose word is not zero.
struct Block {
    words: Vec<u64>,
    touched: Vec<u32>,
}

impl Block {
    /// Evaluates each query that one of this block's terms can trigger, then clears the block.
    /// A query can have several terms in its cover, so `last` makes sure it runs only once.
    fn flush(&mut self, plan: &Compiled, totals: &mut Totals, block: u16) {
        for &term in &self.touched {
            for &query in plan.triggers.get(term) {
                let q = query as usize;
                if totals.last[q] != block {
                    totals.last[q] = block;
                    let result = evaluate(&plan.queries[q], &self.words);
                    if result != 0 {
                        totals.add_word(q, result, block as u32 * 64);
                    }
                }
            }
        }
        for &term in &self.touched {
            self.words[term as usize] = 0;
        }
        self.touched.clear();
    }
}

/// Checks the phrases anchored at position `at` of a document's token ids. The ids are those
/// of this document alone, and separators are not in them, so a phrase cannot cross into
/// another document and any punctuation between its terms is invisible.
#[inline]
fn check_phrases(plan: &Compiled, ids: &[u32], at: usize, doc: u16, totals: &mut Totals) {
    for anchor in plan.anchors.get(ids[at]) {
        let q = anchor.query as usize;
        let query = &plan.queries[q];
        let Some(start) = at.checked_sub(anchor.at as usize) else {
            continue;
        };
        if totals.last[q] != doc && ids[start..].starts_with(&query.terms[..query.len]) {
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
    let mut block = Block {
        words: vec![0; plan.table.len()],
        touched: Vec::with_capacity(plan.table.len()),
    };
    let (mut lower, mut ends, mut ids, mut anchored) = (vec![], vec![], vec![], vec![]);
    let (mut at, mut doc) = (0, 0u16);
    while at < shard.len() {
        assert!(doc != crate::eval::NEVER, "too many documents in one shard");
        let (_, tab) = parse_id(shard, at);
        let line_end = tokenize(shard, tab + 1, &mut lower, &mut ends);
        at = line_end + 1;

        ids.clear();
        anchored.clear();
        let mut start = 0;
        for &end in &ends {
            let id = plan.table.lookup(&lower[start..end as usize]);
            start = end as usize;
            if id != NONE {
                let word = &mut block.words[id as usize];
                if *word == 0 {
                    block.touched.push(id);
                }
                *word |= 1 << (doc % 64);
                if plan.anchors.has(id) {
                    anchored.push(ids.len());
                }
            }
            ids.push(id);
        }
        for &position in &anchored {
            check_phrases(plan, &ids, position, doc, &mut totals);
        }

        if doc % 64 == 63 {
            block.flush(plan, &mut totals, doc / 64);
        }
        doc = doc.wrapping_add(1);
    }
    if doc % 64 != 0 {
        block.flush(plan, &mut totals, doc / 64);
    }
    totals
}
