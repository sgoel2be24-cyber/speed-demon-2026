//! Joins the shards' totals into `results.txt`.

use crate::eval::{MAX_IDS, Totals};
use std::fmt::Write;

/// One line per query, in the order of `queries.txt`. Shards are in document order, so taking
/// ids from them in turn gives the smallest ids, ascending.
pub fn format_results(shards: &[Totals], position_of_line: &[u32]) -> String {
    let mut out = String::with_capacity(position_of_line.len() * 32);
    for &position in position_of_line {
        let q = position as usize;
        let mut count = 0u64;
        let mut sum = 0u64;
        for shard in shards {
            let tally = shard.tally[q];
            let n = tally.count as u64;
            count += n;
            sum += tally.sum as u64 + n * shard.base as u64;
        }
        write!(out, "{count} {sum}").expect("writing to a String");
        let mut wanted = MAX_IDS;
        for shard in shards {
            let take = wanted.min(shard.tally[q].count as usize);
            for &doc in &shard.first[q][..take] {
                write!(out, " {}", shard.base + doc as u32).expect("writing to a String");
            }
            wanted -= take;
        }
        out.push('\n');
    }
    out
}
