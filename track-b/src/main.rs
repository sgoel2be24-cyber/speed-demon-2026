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

use std::ffi::{c_int, c_void};
use std::fs::{self, File};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::{env, io, slice, thread};

/// Using fewer shards than this could overflow the 16-bit local document numbers.
const MIN_SHARDS: usize = 12;

const PROT_READ: c_int = 1;
const MAP_PRIVATE: c_int = 2;

unsafe extern "C" {
    fn mmap(
        addr: *mut c_void,
        len: usize,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        off: i64,
    ) -> *mut c_void;
}

/// Maps a file read-only. The harness loads `/data` into the page cache before the run, so
/// these pages are not charged to this program's memory.
fn map_file(path: &Path) -> io::Result<&'static [u8]> {
    let file = File::open(path)?;
    let len = file.metadata()?.len() as usize;
    // SAFETY: a fresh private read-only mapping aliases nothing in this program, and it is
    // never unmapped, so the 'static slice stays valid. `/data` does not change during a run.
    unsafe {
        let start = mmap(
            std::ptr::null_mut(),
            len,
            PROT_READ,
            MAP_PRIVATE,
            file.as_raw_fd(),
            0,
        );
        if start as isize == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(slice::from_raw_parts(start as *const u8, len))
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
    let threads = args.get(3).map_or_else(
        || {
            thread::available_parallelism()
                .map_or(1, |n| n.get())
                .max(MIN_SHARDS)
        },
        |s| s.parse().expect("thread count"),
    );

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

    fs::write(
        out.join("results.txt"),
        output::format_results(&totals, plan.queries.len()),
    )
}
