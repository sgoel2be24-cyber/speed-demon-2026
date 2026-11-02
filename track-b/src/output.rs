//! Joins the shards' totals into `results.txt`.

use crate::eval::{MAX_IDS, Totals};

/// Appends `n` in decimal.
fn push_number(out: &mut Vec<u8>, mut n: u64) {
    let mut digits = [0; 20];
    let mut at = digits.len();
    loop {
        at -= 1;
        digits[at] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    out.extend_from_slice(&digits[at..]);
}

/// One line per query, in the order of `queries.txt`. Shards are in document order, so taking
/// ids from them in turn gives the smallest ids, ascending.
pub fn format_results(shards: &[Totals], position_of_line: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(position_of_line.len() * 32);
    for &position in position_of_line {
        let q = position as usize;
        let mut count = 0u64;
        let mut sum = 0u64;
        for shard in shards {
            let n = shard.count[q] as u64;
            count += n;
            sum += shard.sum[q] as u64 + n * shard.base as u64;
        }
        push_number(&mut out, count);
        out.push(b' ');
        push_number(&mut out, sum);
        let mut wanted = MAX_IDS;
        for shard in shards {
            let take = wanted.min(shard.count[q] as usize);
            for rank in 0..take {
                let doc = shard.first[rank * shard.count.len() + q];
                out.push(b' ');
                push_number(&mut out, (shard.base + doc as u32) as u64);
            }
            wanted -= take;
        }
        out.push(b'\n');
    }
    out
}
