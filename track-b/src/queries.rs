//! Parses `queries.txt` and works out, before any document is read, when each query has
//! to be looked at during the scan.

use crate::terms::TermTable;
use std::cmp::Reverse;

/// The spec's limit: a phrase has up to 8 terms, a BOOL query up to 6.
pub const MAX_TERMS: usize = 8;

/// A query whose cover terms appear this often in `queries.txt` is evaluated in every block,
/// with no trigger lists, because it would be triggered in nearly all of them. This affects
/// speed only, never an answer: evaluating more often cannot change a result.
const ALWAYS_ON_USES: u64 = 16;

#[derive(Clone, Copy, Default)]
pub enum Op {
    #[default]
    And,
    Or,
    Not,
}

/// One line of `queries.txt`, with its terms as ids. `ops[i]` joins `terms[i]` and
/// `terms[i + 1]`. A phrase has no operators.
#[derive(Clone, Copy, Default)]
pub struct Query {
    pub terms: [u32; MAX_TERMS],
    pub ops: [Op; 6],
    pub len: u8,
    pub is_phrase: bool,
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
}

/// Everything the scan needs to know about the queries.
pub struct Compiled<'a> {
    pub table: TermTable<'a>,
    /// The queries, BOOL first, then from the most often evaluated to the least, so the ones
    /// evaluated in nearly every block sit together in memory. Phrases follow.
    pub queries: Vec<Query>,
    pub n_bool: usize,
    /// How many terms appear in a BOOL query. These have the lowest ids.
    pub n_bool_terms: u32,
    /// How many of the first queries run in every block.
    pub n_always: usize,
    /// For each line of `queries.txt`, its position in `queries`.
    pub position_of_line: Vec<u32>,
    /// For each term, the BOOL queries to evaluate in a block where the term occurs.
    pub triggers: TermLists<u32>,
    /// For each term, the phrases to check at each of its occurrences, as the query and the
    /// position of the term within the phrase.
    pub anchors: TermLists<(u32, u32)>,
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
/// expected, so a word is an operator only because of where it stands. Only a phrase has
/// quotes, and they cannot appear anywhere else.
fn parse_line<'a>(line: &'a [u8], table: &mut TermTable<'a>) -> Query {
    let space = line.iter().position(|&b| b == b' ').expect("query kind");
    let rest = &line[space + 1..];
    let quoted = rest.strip_prefix(b"\"");
    let body = quoted.map_or(rest, |text| &text[..text.len() - 1]);
    let mut query = Query {
        is_phrase: quoted.is_some(),
        ..Query::default()
    };
    for (i, word) in body.split(|&b| b == b' ').enumerate() {
        if query.is_phrase || i % 2 == 0 {
            query.terms[query.len as usize] = table.intern(word);
            query.len += 1;
        } else {
            query.ops[query.len as usize - 1] = parse_op(word);
        }
    }
    query
}

/// Adds the query's cover to `pairs`: a set of terms such that every matching document
/// contains at least one of them. The cover decides when to evaluate, never the answer.
///
/// By induction over the left-to-right evaluation, with R the result so far:
/// a match of `R AND t` contains `t` and is in R, so either side's cover will do;
/// a match of `R OR t` is in R or contains `t`, so both covers are needed;
/// a match of `R NOT t` is in R, so R's cover is enough.
fn push_cover(query: &Query, index: u32, freq: &[u64], pairs: &mut Vec<(u32, u32)>) {
    let start = pairs.len();
    pairs.push((query.terms[0], index));
    for i in 1..query.len as usize {
        let term = query.terms[i];
        match query.ops[i - 1] {
            Op::And => {
                let cost: u64 = pairs[start..].iter().map(|p| freq[p.0 as usize]).sum();
                if freq[term as usize] < cost {
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
fn pick_anchor(query: &Query, table: &TermTable, freq: &[u64]) -> usize {
    (0..query.len as usize)
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
    for line in text.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
        queries.push(parse_line(line, &mut table));
    }

    // Number terms from most to least used, so the busiest term words share cache lines, with
    // terms that only phrases use after the rest, because those need no word at all.
    let mut uses = vec![0u64; table.len()];
    let mut in_bool = vec![false; table.len()];
    for query in &queries {
        for &term in &query.terms[..query.len as usize] {
            uses[term as usize] += 1;
            in_bool[term as usize] |= !query.is_phrase;
        }
    }
    let mut by_use: Vec<u32> = (0..table.len() as u32).collect();
    by_use.sort_unstable_by_key(|&t| (!in_bool[t as usize], Reverse(uses[t as usize]), t));
    let n_bool_terms = in_bool.iter().filter(|&&used| used).count() as u32;
    let mut new_id = vec![0u32; table.len()];
    let mut freq = vec![0u64; table.len()];
    for (new, &old) in by_use.iter().enumerate() {
        new_id[old as usize] = new as u32;
        freq[new] = uses[old as usize];
    }
    table.renumber(&new_id);
    for term in queries.iter_mut().flat_map(|query| &mut query.terms) {
        *term = new_id[*term as usize];
    }

    let mut cover_pairs = Vec::new();
    let mut anchor_pairs = Vec::new();
    let mut evaluations = vec![0; queries.len()];
    for (line, query) in queries.iter().enumerate() {
        if query.is_phrase {
            let at = pick_anchor(query, &table, &freq);
            anchor_pairs.push((query.terms[at], (line as u32, at as u32)));
            table.mark_anchor(query.terms[at]);
        } else {
            let start = cover_pairs.len();
            push_cover(query, line as u32, &freq, &mut cover_pairs);
            let cover = &cover_pairs[start..];
            evaluations[line] = cover.iter().map(|p| freq[p.0 as usize]).sum::<u64>() + 1;
        }
    }

    let mut lines: Vec<u32> = (0..queries.len() as u32).collect();
    lines.sort_unstable_by_key(|&line| (Reverse(evaluations[line as usize]), line));
    let mut position_of_line = vec![0; lines.len()];
    for (position, &line) in lines.iter().enumerate() {
        position_of_line[line as usize] = position as u32;
    }
    let n_bool = evaluations.iter().filter(|&&e| e > 0).count();
    let queries = lines.iter().map(|&line| queries[line as usize]).collect();
    for pair in &mut cover_pairs {
        pair.1 = position_of_line[pair.1 as usize];
    }
    let n_always = evaluations.iter().filter(|&&e| e > ALWAYS_ON_USES).count();
    cover_pairs.retain(|pair| pair.1 as usize >= n_always);
    for pair in &mut anchor_pairs {
        pair.1.0 = position_of_line[pair.1.0 as usize];
    }

    // One more list than terms, always empty, for the id that misses get.
    let n_terms = table.len() + 1;
    Compiled {
        table,
        queries,
        n_bool,
        n_bool_terms,
        n_always,
        position_of_line,
        triggers: TermLists::build(n_terms, &cover_pairs),
        anchors: TermLists::build(n_terms, &anchor_pairs),
    }
}
