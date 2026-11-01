//! Speed Demon 2026, Track B. Answers 50,000 BOOL and PHRASE queries in one pass over the
//! corpus, with no index. See README.md for how and why.
//!
//!     solution [data_dir] [out_dir] [threads]

mod classify;
mod eval;
mod output;
mod queries;
mod scan;
mod terms;

use std::fs::{self, File};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::ptr::null_mut;
use std::{env, io, slice, thread};

/// Using fewer shards than this could overflow the 16-bit local document numbers.
const MIN_SHARDS: usize = 12;

const PROT_READ: i32 = 1;
const MAP_PRIVATE: i32 = 2;

unsafe extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
}

/// Maps a file read-only. The harness loads `/data` into the page cache before the run, so
/// these pages are not charged to this program's memory.
fn map_file(path: &Path) -> io::Result<&'static [u8]> {
    let file = File::open(path)?;
    let len = file.metadata()?.len() as usize;
    // SAFETY: a fresh private read-only mapping aliases nothing in this program, and it is
    // never unmapped, so the 'static slice stays valid. `/data` does not change during a run.
    unsafe {
        let start = mmap(null_mut(), len, PROT_READ, MAP_PRIVATE, file.as_raw_fd(), 0);
        if start as isize == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(slice::from_raw_parts(start, len))
    }
}

/// Cuts the file into `count` parts of about equal size, each ending just after an LF.
fn shards(bytes: &[u8], count: usize) -> Vec<&[u8]> {
    let mut parts = Vec::with_capacity(count);
    let mut start = 0;
    for i in 1..=count {
        let target = bytes.len() * i / count;
        let end = bytes[target.max(start + 1) - 1..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(bytes.len(), |lf| target.max(start + 1) + lf);
        if end > start {
            parts.push(&bytes[start..end]);
            start = end;
        }
    }
    parts
}

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    let data = Path::new(args.get(1).map_or("/data", |s| s));
    let out = Path::new(args.get(2).map_or("/out", |s| s));
    let cores = thread::available_parallelism().map_or(1, |n| n.get());
    let threads = args
        .get(3)
        .map_or(cores.max(MIN_SHARDS), |s| s.parse().expect("thread count"));

    let docs = map_file(&data.join("docs.txt"))?;
    let plan = queries::compile(map_file(&data.join("queries.txt"))?);

    let totals: Vec<_> = thread::scope(|scope| {
        let plan = &plan;
        let workers: Vec<_> = shards(docs, threads)
            .into_iter()
            .map(|shard| scope.spawn(move || scan::scan_shard(plan, shard)))
            .collect();
        workers
            .into_iter()
            .map(|w| w.join().expect("worker"))
            .collect()
    });

    let results = output::format_results(&totals, &plan.position_of_line);
    fs::write(out.join("results.txt"), results)
}
