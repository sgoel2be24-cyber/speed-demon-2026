//! All memory comes from one block of huge pages. The scan reads several megabytes of tables
//! at random, which on 4 KB pages misses the TLB as well as the caches. Nothing is ever freed:
//! the program runs once and exits, and the few buffers that grow waste little.

use std::alloc::{GlobalAlloc, Layout};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

const HUGE_PAGE: usize = 1 << 21;
/// Address space only: memory counts as used when it is first touched.
const SPACE: usize = 1 << 30;

const PROT_READ_WRITE: i32 = 3;
#[cfg(target_os = "linux")]
const MAP_PRIVATE_ANONYMOUS: i32 = 0x22;
#[cfg(not(target_os = "linux"))]
const MAP_PRIVATE_ANONYMOUS: i32 = 0x1002;
const MADV_HUGEPAGE: i32 = 14;

unsafe extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn madvise(addr: *mut u8, len: usize, advice: i32) -> i32;
}

/// The next free address, or 0 before the first allocation.
static NEXT: AtomicUsize = AtomicUsize::new(0);

pub struct Arena;

/// Reserves the space, starting on a huge page. The runtime makes its first allocation on the
/// main thread, before any other thread exists, so this runs exactly once.
unsafe fn reserve() -> usize {
    unsafe {
        let start = mmap(null_mut(), SPACE, PROT_READ_WRITE, MAP_PRIVATE_ANONYMOUS, -1, 0);
        assert!(start as isize != -1, "reserving memory");
        let aligned = (start as usize).next_multiple_of(HUGE_PAGE);
        madvise(aligned as *mut u8, SPACE - HUGE_PAGE, MADV_HUGEPAGE);
        aligned
    }
}

// SAFETY: each allocation is a distinct range of a mapping that is never unmapped, aligned as
// asked. The mapping starts out zeroed and no range is used twice, so zeroed requests hold.
unsafe impl GlobalAlloc for Arena {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if NEXT.load(Relaxed) == 0 {
            NEXT.store(unsafe { reserve() }, Relaxed);
        }
        let padded = layout.size() + layout.align() - 1;
        let at = NEXT.fetch_add(padded, Relaxed);
        at.next_multiple_of(layout.align()) as *mut u8
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        unsafe { self.alloc(layout) }
    }

    unsafe fn dealloc(&self, _: *mut u8, _: Layout) {}
}
