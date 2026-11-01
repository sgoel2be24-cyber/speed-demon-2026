//! Parses `queries.txt` and works out, before any document is read, when each query has
//! to be looked at during the scan.

use crate::terms::TermTable;

/// The spec's limit: a phrase has up to 8 terms, a BOOL query up to 6.
pub const MAX_TERMS: usize = 8;

#[derive(Clone, Copy)]
pub enum Op {
    And,
    Or,
    Not,
}

/// One line of `queries.txt`, with its terms as ids. `ops[i]` joins `terms[i]` and
/// `terms[i + 1]`. A phrase has no operators.
#[derive(Clone, Copy)]
pub struct Query {
    pub terms: [u32; MAX_TERMS],
    pub ops: [Op; MAX_TERMS],
    pub len: usize,
}

/// A phrase to check wherever its anchor term occurs. The anchor is term `at` of the phrase.
#[derive(Clone, Copy)]
pub struct Anchor {
    pub query: u32,
    pub at: u32,
}

/// A compact map from term id to a list of items.
pub struct TermLists<T> {
    offsets: Vec<u32>,
    items: Vec<T>,
}

impl<T: Copy> TermLists<T> {
    fn build(n_terms: usize, pairs: &[(u32, T)]) -> Self {
        let mut offsets = vec![0u32; n_terms + 2];
        for &(term, _) in pairs {
            offsets[term as usize + 2] += 1;
        }
        for i in 2..offsets.len() {
            offsets[i] += offsets[i - 1];
        }
        // Each pair moves its term's start forward, leaving offsets[t]..offsets[t + 1] filled.
        let mut items: Vec<T> = pairs.iter().map(|&(_, item)| item).collect();
        for &(term, item) in pairs {
            let next = &mut offsets[term as usize + 1];
            items[*next as usize] = item;
            *next += 1;
        }
        offsets.pop();
        TermLists { offsets, items }
    }

    #[inline]
    pub fn get(&self, term: u32) -> &[T] {
        let t = term as usize;
        &self.items[self.offsets[t] as usize..self.offsets[t + 1] as usize]
    }

    #[inline]
    pub fn has(&self, term: u32) -> bool {
        let t = term as usize;
        self.offsets[t] != self.offsets[t + 1]
    }
}

/// Everything the scan needs to know about the queries.
pub struct Compiled<'a> {
    pub table: TermTable<'a>,
    pub queries: Vec<Query>,
    /// For each term, the BOOL queries to evaluate in a block where the term occurs.
    pub triggers: TermLists<u32>,
    /// For each term, the phrases to check at each of its occurrences.
    pub anchors: TermLists<Anchor>,
}

fn parse_op(word: &[u8]) -> Op {
    match word {
        b"AND" => Op::And,
        b"OR" => Op::Or,
        b"NOT" => Op::Not,
        _ => panic!("unknown operator {:?}", String::from_utf8_lossy(word)),
    }
}

/// Reads one line by position: `and`, `or` and `not` are ordinary terms wherever a term is
/// expected, so a word is an operator only because of where it stands.
fn parse_line<'a>(line: &'a [u8], table: &mut TermTable<'a>) -> (Query, bool) {
    let space = line.iter().position(|&b| b == b' ').expect("query kind");
    let (kind, rest) = (&line[..space], &line[space + 1..]);
    let is_phrase = kind == b"PHRASE";
    let body = if is_phrase {
        &rest[1..rest.len() - 1]
    } else {
        rest
    };
    let mut query = Query {
        terms: [0; MAX_TERMS],
        ops: [Op::And; MAX_TERMS],
        len: 0,
    };
    for (i, word) in body.split(|&b| b == b' ').enumerate() {
        if is_phrase || i % 2 == 0 {
            query.terms[query.len] = table.intern(word);
            query.len += 1;
        } else {
            query.ops[query.len - 1] = parse_op(word);
        }
    }
    (query, is_phrase)
}

/// Adds the query's cover to `pairs`: a set of terms such that every matching document
/// contains at least one of them. The cover decides when to evaluate, never the answer.
///
/// By induction over the left-to-right evaluation, with R the result so far:
/// a match of `R AND t` contains `t` and is in R, so either side's cover will do;
/// a match of `R OR t` is in R or contains `t`, so both covers are needed;
/// a match of `R NOT t` is in R, so R's cover is enough.
fn push_cover(query: &Query, index: u32, freq: &[u32], pairs: &mut Vec<(u32, u32)>) {
    let start = pairs.len();
    pairs.push((query.terms[0], index));
    for i in 1..query.len {
        let term = query.terms[i];
        match query.ops[i - 1] {
            Op::And => {
                let cost: u64 = pairs[start..]
                    .iter()
                    .map(|&(t, _)| freq[t as usize] as u64)
                    .sum();
                if (freq[term as usize] as u64) < cost {
                    pairs.truncate(start);
                    pairs.push((term, index));
                }
            }
            Op::Or => pairs.push((term, index)),
            Op::Not => {}
        }
    }
}

/// The phrase term to anchor on. Any term is correct, so this only affects speed. How often
/// a term appears in `queries.txt` is a good, free guide to how rare it is in the corpus.
fn pick_anchor(query: &Query, table: &TermTable, freq: &[u32]) -> usize {
    (0..query.len)
        .min_by_key(|&i| {
            let term = query.terms[i];
            (freq[term as usize], usize::MAX - table.term(term).len())
        })
        .expect("a phrase has terms")
}

pub fn compile(text: &[u8]) -> Compiled<'_> {
    let words = text.iter().filter(|&&b| b == b' ' || b == b'\n').count();
    let mut table = TermTable::with_capacity(words);
    let mut queries = Vec::new();
    let mut is_phrase = Vec::new();
    for line in text.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
        let (query, phrase) = parse_line(line, &mut table);
        queries.push(query);
        is_phrase.push(phrase);
    }

    // Number terms from most to least used, so the busiest term words share cache lines.
    let mut uses = vec![0u32; table.len()];
    for query in &queries {
        for &term in &query.terms[..query.len] {
            uses[term as usize] += 1;
        }
    }
    let mut by_use: Vec<u32> = (0..table.len() as u32).collect();
    by_use.sort_by_key(|&term| std::cmp::Reverse(uses[term as usize]));
    let mut new_id = vec![0u32; table.len()];
    let mut freq = vec![0u32; table.len()];
    for (new, &old) in by_use.iter().enumerate() {
        new_id[old as usize] = new as u32;
        freq[new] = uses[old as usize];
    }
    table.renumber(&new_id);
    for query in &mut queries {
        let len = query.len;
        for term in &mut query.terms[..len] {
            *term = new_id[*term as usize];
        }
    }

    let mut cover_pairs = Vec::new();
    let mut anchor_pairs = Vec::new();
    for (i, query) in queries.iter().enumerate() {
        if is_phrase[i] {
            let at = pick_anchor(query, &table, &freq);
            let anchor = Anchor {
                query: i as u32,
                at: at as u32,
            };
            anchor_pairs.push((query.terms[at], anchor));
        } else {
            push_cover(query, i as u32, &freq, &mut cover_pairs);
        }
    }
    let n_terms = table.len();
    Compiled {
        table,
        queries,
        triggers: TermLists::build(n_terms, &cover_pairs),
        anchors: TermLists::build(n_terms, &anchor_pairs),
    }
}
