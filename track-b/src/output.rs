//! Joins the shards' totals into `results.txt`.

use crate::eval::{MAX_IDS, Totals};
use std::fmt::Write;

/// One line per query, in query order. Shards are in document order, so taking ids from them
/// in turn gives the smallest ids, ascending.
pub fn format_results(shards: &[Totals], n_queries: usize) -> String {
    let mut out = String::with_capacity(n_queries * 32);
    for q in 0..n_queries {
        let mut count = 0u64;
        let mut sum = 0u64;
        for shard in shards {
            let n = shard.count[q] as u64;
            count += n;
            sum += shard.sum[q] as u64 + n * shard.base as u64;
        }
        write!(out, "{count} {sum}").expect("writing to a String");
        let mut wanted = MAX_IDS;
        for shard in shards {
            let take = wanted.min(shard.count[q] as usize);
            for &doc in &shard.first[q][..take] {
                write!(out, " {}", shard.base + doc as u32).expect("writing to a String");
            }
            wanted -= take;
        }
        out.push('\n');
    }
    out
}
