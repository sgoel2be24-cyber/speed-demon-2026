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

const HUGE_PAGE: usize = 1 << 21;
const PROT_READ_WRITE: i32 = 3;
const MADV_HUGEPAGE: i32 = 14;
#[cfg(target_os = "linux")]
const MAP_PRIVATE_ANONYMOUS: i32 = 0x22;
#[cfg(not(target_os = "linux"))]
const MAP_PRIVATE_ANONYMOUS: i32 = 0x1002;

unsafe extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn munmap(addr: *mut u8, len: usize) -> i32;
    fn madvise(addr: *mut u8, len: usize, advice: i32) -> i32;
}

/// A filled slice on huge pages, where the system allows them. The term table is read at
/// random, which on 4 KB pages would miss the TLB as well as the caches, and giving it huge
/// pages gains most of what giving them to everything does, for far less memory.
pub fn huge_slice<T: Copy>(len: usize, value: T) -> &'static mut [T] {
    let bytes = (len * size_of::<T>()).next_multiple_of(HUGE_PAGE);
    // SAFETY: a fresh anonymous mapping aliases nothing and stays valid until `free_huge`. It
    // is large enough to start on a huge page, which is aligned for any `T` used here.
    unsafe {
        let flags = MAP_PRIVATE_ANONYMOUS;
        let start = mmap(null_mut(), bytes + HUGE_PAGE, PROT_READ_WRITE, flags, -1, 0);
        assert!(start as isize != -1, "reserving memory");
        let aligned = (start as usize).next_multiple_of(HUGE_PAGE) as *mut T;
        madvise(aligned.cast(), bytes, MADV_HUGEPAGE);
        let slice = slice::from_raw_parts_mut(aligned, len);
        slice.fill(value);
        slice
    }
}

/// Gives back a slice from `huge_slice`, which must not be used again.
pub fn free_huge<T>(slice: &mut [T]) {
    let bytes = size_of_val(slice).next_multiple_of(HUGE_PAGE);
    // SAFETY: the range was mapped by `huge_slice` and the caller drops the slice.
    unsafe { munmap(slice.as_mut_ptr().cast(), bytes) };
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

    let lines = &plan.position_of_line;
    let parts: Vec<Vec<u8>> = thread::scope(|scope| {
        let totals = &totals;
        let workers: Vec<_> = lines
            .chunks(lines.len().div_ceil(threads).max(1))
            .map(|part| scope.spawn(move || output::format_results(totals, part)))
            .collect();
        workers
            .into_iter()
            .map(|w| w.join().expect("worker"))
            .collect()
    });
    fs::write(out.join("results.txt"), parts.concat())
}
