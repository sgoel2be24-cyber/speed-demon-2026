//! Parses `queries.txt` and works out, before any document is read, when each query has
//! to be looked at during the scan.

use crate::terms::TermTable;
use std::cmp::Reverse;

/// The spec's limit: a phrase has up to 8 terms, a BOOL query up to 6.
pub const MAX_TERMS: usize = 8;

/// A BOOL term used at least this often in `queries.txt` is dense: it gets a place in a small
/// array that stays in cache. How often a term appears in the file is a good, free guide to
/// how common it is in the corpus, and this affects speed only, never an answer.
const DENSE_USES: u64 = 3;

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

/// What a BOOL query becomes in a block that holds none of its rare terms: its dense terms only.
#[derive(Clone, Copy, Default)]
pub struct Form {
    pub terms: [u16; 6],
    pub ops: [Op; 5],
    pub len: u8,
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
    /// The queries: those with a folded form first, the other BOOL queries from the most often
    /// evaluated to the least, then the phrases.
    pub queries: Vec<Query>,
    pub n_bool: usize,
    /// The folded forms of the first `forms.len()` queries, which run in every block.
    pub forms: Vec<Form>,
    /// How many terms appear in a BOOL query. These have the lowest ids.
    pub n_bool_terms: u32,
    /// How many terms are dense. These have the lowest ids of all.
    pub n_dense: u32,
    /// For each line of `queries.txt`, its position in `queries`.
    pub position_of_line: Vec<u32>,
    /// For each rare term, the BOOL queries to evaluate in full in a block where it occurs.
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

/// The query with every rare term taken as absent, which is its exact meaning in any block
/// that holds none of them. An empty form means every match contains a rare term.
fn fold(query: &Query, n_dense: u32) -> Form {
    let mut form = Form::default();
    for i in 0..query.len as usize {
        let term = query.terms[i];
        let op = if i == 0 { Op::Or } else { query.ops[i - 1] };
        let len = form.len as usize;
        match (term < n_dense, op) {
            (false, Op::And) => form.len = 0,
            (false, _) => {}
            (true, Op::Or) if len == 0 => {
                form.terms[0] = term as u16;
                form.len = 1;
            }
            (true, _) if len == 0 => {}
            (true, _) => {
                form.terms[len] = term as u16;
                form.ops[len - 1] = op;
                form.len += 1;
            }
        }
    }
    form
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
    let n_dense = (0..table.len())
        .filter(|&t| in_bool[t] && uses[t] >= DENSE_USES)
        .count()
        .min(u16::MAX as usize) as u32;
    for term in queries.iter_mut().flat_map(|query| &mut query.terms) {
        *term = new_id[*term as usize];
    }

    let mut cover_pairs = Vec::new();
    let mut anchor_pairs = Vec::new();
    let mut evaluations = vec![0; queries.len()];
    let mut is_folded = vec![false; queries.len()];
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
            // Only rare terms are watched for. A query with a form runs anyway and needs its
            // full version wherever one of them occurs. A query with no form can only match
            // where one of them occurs, so they are a cover too.
            is_folded[line] = fold(query, n_dense).len > 0;
            if is_folded[line] || cover.iter().any(|p| p.0 < n_dense) {
                cover_pairs.truncate(start);
                for &term in &query.terms[..query.len as usize] {
                    if term >= n_dense {
                        cover_pairs.push((term, line as u32));
                    }
                }
            }
        }
    }

    let mut lines: Vec<u32> = (0..queries.len() as u32).collect();
    lines.sort_unstable_by_key(|&line| {
        let line_at = line as usize;
        (!is_folded[line_at], Reverse(evaluations[line_at]), line)
    });
    let mut position_of_line = vec![0; lines.len()];
    for (position, &line) in lines.iter().enumerate() {
        position_of_line[line as usize] = position as u32;
    }
    let n_bool = evaluations.iter().filter(|&&e| e > 0).count();
    let queries: Vec<Query> = lines.iter().map(|&line| queries[line as usize]).collect();
    let n_folded = is_folded.iter().filter(|&&folded| folded).count();
    let forms = queries[..n_folded].iter().map(|q| fold(q, n_dense)).collect();
    for pair in &mut cover_pairs {
        pair.1 = position_of_line[pair.1 as usize];
    }
    for pair in &mut anchor_pairs {
        pair.1.0 = position_of_line[pair.1.0 as usize];
    }

    // One more list than terms, always empty, for the id that misses get.
    let n_terms = table.len() + 1;
    Compiled {
        table,
        queries,
        n_bool,
        forms,
        n_bool_terms,
        n_dense,
        position_of_line,
        triggers: TermLists::build(n_terms, &cover_pairs),
        anchors: TermLists::build(n_terms, &anchor_pairs),
    }
}
