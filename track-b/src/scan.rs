//! One pass over a shard of `docs.txt`, answering every query as it goes.

use crate::classify::{CHUNK, classify};
use crate::eval::prefetch;
use crate::eval::{BLOCK, Totals, Word, evaluate, evaluate_form};
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

/// How many queries ahead a flush starts loading what a query needs.
const AHEAD: usize = 8;

/// The BOOL terms of the current block. `slots[t]` says which entry of `words` holds term `t`.
/// A dense term always has entry `t`. A rare term has `absent`, which stays all zero, until it
/// first occurs in the block and takes the next free entry. Such a term is listed in `rare`,
/// and each token writes to the next free place there, so `rare` needs one more place than
/// there are rare terms. Misses and terms with no BOOL query all share one entry, which nothing
/// reads. `pending` is a bitset of BOOL queries, kept here so that a flush needs no allocation.
struct Block {
    words: Vec<Word>,
    slots: Vec<u32>,
    rare: Vec<u32>,
    n_rare: usize,
    pending: Vec<u64>,
    queue: Vec<u32>,
    absent: u32,
}

impl Block {
    fn new(plan: &Compiled) -> Self {
        let (n_dense, n_bool_terms) = (plan.n_dense, plan.n_bool_terms);
        let absent = n_dense + 1;
        let mut slots: Vec<u32> = (0..n_dense).collect();
        slots.resize(n_bool_terms as usize, absent);
        slots.push(n_dense);
        Block {
            words: vec![[0; 4]; n_bool_terms as usize + 3],
            slots,
            rare: vec![0; (n_bool_terms - n_dense) as usize + 2],
            n_rare: 0,
            pending: vec![0; plan.n_bool / 64 + 1],
            queue: vec![],
            absent,
        }
    }

    /// Runs every query this block can affect, then clears the block.
    ///
    /// A query with a folded form runs on it unless one of its rare terms occurred. Those, and
    /// the queries with no form, are marked in a bitset first, so a query runs only once.
    fn flush(&mut self, plan: &Compiled, totals: &mut Totals, block: u32) {
        let start = block * BLOCK as u32;
        for &term in &self.rare[..self.n_rare] {
            for &query in plan.triggers.get(term) {
                self.pending[query as usize / 64] |= 1 << (query % 64);
            }
        }
        for (q, form) in plan.forms.iter().enumerate() {
            if self.pending[q / 64] >> (q % 64) & 1 == 0 {
                totals.add_block(q, &evaluate_form(form, &self.words), start);
            }
        }
        self.queue.clear();
        for (i, pending) in self.pending.iter_mut().enumerate() {
            let mut queries = std::mem::take(pending);
            while queries != 0 {
                self.queue.push((i * 64) as u32 + queries.trailing_zeros());
                queries &= queries - 1;
            }
        }
        // These read scattered memory, so each one's records start loading well before its
        // turn: what a query holds first, then the entries of the terms it names.
        for (i, &q) in self.queue.iter().enumerate() {
            if let Some(&far) = self.queue.get(i + 2 * AHEAD) {
                prefetch(&plan.queries[far as usize]);
                totals.prefetch(far as usize);
            }
            if let Some(&near) = self.queue.get(i + AHEAD) {
                let query = &plan.queries[near as usize];
                for &term in &query.terms[..query.len as usize] {
                    prefetch(&self.slots[term as usize]);
                }
            }
            let result = evaluate(&plan.queries[q as usize], &self.words, &self.slots);
            if result != [0; 4] {
                totals.add_block(q as usize, &result, start);
            }
        }
        let first_rare = self.absent as usize + 1;
        self.words[..plan.n_dense as usize].fill([0; 4]);
        self.words[first_rare..first_rare + self.n_rare].fill([0; 4]);
        for &term in &self.rare[..self.n_rare] {
            self.slots[term as usize] = self.absent;
        }
        self.n_rare = 0;
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

/// Turns tokens into term ids and sets each term's bit for document `doc`. A dense term needs
/// one write. Rare terms have a place only while they are in the block, and those places are
/// scattered, so they wait in `waiting` until the whole document can be handled in one batch.
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn set_words(
    plan: &Compiled,
    shard: &[u8],
    tokens: &[[u32; 2]],
    keys: &[(usize, Key)],
    ids: &mut [u32],
    waiting: &mut [u32],
    block: &mut Block,
    anchored: &mut Vec<usize>,
    doc: u16,
) {
    let (part, bit) = (doc as usize / 64 % 4, 1u64 << (doc % 64));
    let (table, n_dense, n_bool_terms) = (&plan.table, plan.n_dense, plan.n_bool_terms);
    let (absent, mut n_rare) = (block.absent, block.n_rare);
    let (words, slots, rare) = (&mut block.words[..], &mut block.slots[..], &mut block.rare[..]);
    let first_rare = absent + 1;
    let mut n_waiting = 0;
    for (position, ((id, &[start, end]), &(home, key))) in
        ids.iter_mut().zip(tokens).zip(keys).enumerate()
    {
        let found = table.lookup(home, key, shard, start as usize, end as usize);
        let term = found.id();
        *id = term;
        // SAFETY: `words` has an entry for `n_dense`, which nothing reads, and `waiting` has
        // room for every token of the document.
        unsafe {
            *words.get_unchecked_mut(term.min(n_dense) as usize).get_unchecked_mut(part) |= bit;
            *waiting.get_unchecked_mut(n_waiting) = term;
        }
        n_waiting += (term >= n_dense && term < n_bool_terms) as usize;
        if found.is_anchor() {
            anchored.push(position);
        }
    }
    for i in 0..n_waiting {
        if let Some(&next) = waiting.get(i + AHEAD) {
            prefetch(&slots[next as usize]);
        }
        let term = waiting[i];
        // SAFETY: a waiting term is below `n_bool_terms`, which `slots` covers. A slot is below
        // `first_rare + n_rare`, and `n_rare` is below the size of `rare` because each rare
        // term enters once, so all of these stay inside `words` and `rare`.
        unsafe {
            let entry = slots.get_unchecked_mut(term as usize);
            let is_new = *entry == absent;
            let slot = if is_new { first_rare + n_rare as u32 } else { *entry };
            *entry = slot;
            *rare.get_unchecked_mut(n_rare) = term;
            n_rare += is_new as usize;
            *words.get_unchecked_mut(slot as usize).get_unchecked_mut(part) |= bit;
        }
    }
    block.n_rare = n_rare;
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
    let mut block = Block::new(plan);
    let (mut edges, mut ids, mut anchored, mut keys) = (vec![], vec![], vec![], vec![]);
    let mut waiting = vec![];
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
            waiting.resize(tokens.len(), 0);
            keys.resize(tokens.len(), (0, Key::default()));
        }
        anchored.clear();
        hash_tokens(plan, shard, tokens, &mut keys);
        let (ids, waiting) = (&mut ids, &mut waiting);
        set_words(plan, shard, tokens, &keys, ids, waiting, &mut block, &mut anchored, doc);
        for &position in &anchored {
            check_phrases(plan, &ids[..tokens.len()], position, doc, &mut totals);
        }

        if doc as usize % BLOCK == BLOCK - 1 {
            block.flush(plan, &mut totals, doc as u32 / BLOCK as u32);
        }
        doc = doc.wrapping_add(1);
    }
    if doc as usize % BLOCK != 0 {
        block.flush(plan, &mut totals, doc as u32 / BLOCK as u32);
    }
    totals
}
