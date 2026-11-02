//! One pass over a shard of `docs.txt`, answering every query as it goes.

use crate::classify::{CHUNK, classify};
use crate::eval::{Totals, evaluate};
use crate::queries::Compiled;
use crate::terms::Key;
pub type ShardResult = Totals;

/// Finds where one document's tokens start and stop. `edges` receives the position of each
/// change between token and separator, so tokens are its consecutive pairs. Returns the
/// position of the LF that ends the document, which is what closes a token at the very end.
fn find_token_edges(bytes: &[u8], mut at: usize, edges: &mut Vec<u32>) -> usize {
    edges.clear();
    let mut in_token = 0;
    loop {
        let mut padded = [0; CHUNK];
        // Near the end of the file, a full read would run past the mapping.
        let chunk = match bytes.get(at..at + CHUNK) {
            Some(full) => full.try_into().expect("a full chunk"),
            None => {
                padded[..bytes.len() - at].copy_from_slice(&bytes[at..]);
                &padded
            }
        };
        let (mut tokens, line_ends) = classify(chunk);
        if line_ends != 0 {
            tokens &= (1 << line_ends.trailing_zeros()) - 1;
        }
        let mut changes = tokens ^ (tokens << 1 | in_token);
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

/// Parses the decimal id at the start of a line. Returns the id and the position of its TAB.
fn parse_id(bytes: &[u8], mut at: usize) -> (u32, usize) {
    let mut id = 0;
    while bytes[at] != b'\t' {
        id = id * 10 + (bytes[at] - b'0') as u32;
        at += 1;
    }
    (id, at)
}

/// The BOOL terms of the current block of 64 documents. `words[t]` has bit `i` set if document
/// `i` of the block contains term `t`. The first `n_touched` entries of `touched` list the
/// terms whose word is not zero. A term enters once per block at most, but every token writes
/// to the next free entry, so `touched` needs one entry more than there are ids. `pending`
/// is a bitset of BOOL queries, kept here so that a flush needs no allocation.
struct Block {
    words: Vec<u64>,
    touched: Vec<u32>,
    n_touched: usize,
    pending: Vec<u64>,
}

impl Block {
    /// Evaluates each query that one of this block's terms can trigger, then clears the block.
    ///
    /// Marking the queries in a bitset first means a query with several cover terms runs only
    /// once, and they run in memory order. A flush evaluates thousands of queries, each
    /// touching several arrays, and reading those in order is what keeps the cache misses down.
    fn flush(&mut self, plan: &Compiled, totals: &mut Totals, block: u16) {
        let touched = &self.touched[..self.n_touched];
        for &term in touched {
            for &query in plan.triggers.get(term) {
                self.pending[query as usize / 64] |= 1 << (query % 64);
            }
        }
        for (i, pending) in self.pending.iter_mut().enumerate() {
            let mut queries = std::mem::take(pending);
            while queries != 0 {
                let q = i * 64 + queries.trailing_zeros() as usize;
                queries &= queries - 1;
                let result = evaluate(&plan.queries[q], &self.words);
                if result != 0 {
                    totals.add_word(q, result, block as u32 * 64);
                }
            }
        }
        for &term in touched {
            self.words[term as usize] = 0;
        }
        self.n_touched = 0;
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
    let mut block = Block {
        words: vec![0; plan.n_bool_terms as usize + 1],
        touched: vec![0; plan.n_bool_terms as usize + 2],
        n_touched: 0,
        pending: vec![0; plan.n_bool.div_ceil(64)],
    };
    let (mut edges, mut ids, mut anchored, mut keys) = (vec![], vec![], vec![], vec![]);
    let (mut at, mut doc) = (0, 0u16);
    while at < shard.len() {
        assert!(doc != crate::eval::NEVER, "too many documents in one shard");
        let (_, tab) = parse_id(shard, at);
        let line_end = find_token_edges(shard, tab + 1, &mut edges);
        at = line_end + 1;

        ids.clear();
        ids.resize(edges.len() / 2, 0);
        anchored.clear();
        let tokens = edges.as_chunks::<2>().0;
        keys.clear();
        for &[start, end] in tokens {
            let key = Key::at(shard, start as usize, (end - start) as usize);
            keys.push((plan.table.route(key), key));
        }
        for (position, ((id, &[start, end]), &(home, key))) in
            ids.iter_mut().zip(tokens).zip(&keys).enumerate()
        {
            let (start, end) = (start as usize, end as usize);
            let found = plan.table.lookup(home, key, &shard[start..end]);
            *id = found.id();
            // Written without branches: whether a token is new to the block is unpredictable.
            // Terms with no BOOL query and misses all share one word, which nothing reads.
            let word_id = found.id().min(plan.n_bool_terms);
            let word = &mut block.words[word_id as usize];
            block.touched[block.n_touched] = word_id;
            block.n_touched += (*word == 0) as usize;
            *word |= 1 << (doc % 64);
            if found.is_anchor() {
                anchored.push(position);
            }
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
