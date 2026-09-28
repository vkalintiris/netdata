//! The process's allocator (decisions D87 F5, D91.1 and D91.5 as amended): `System` underneath; each thread knows
//! whether its last allocation failed, so the SIGABRT that Rust's `handle_alloc_error()` raises right after can be
//! recorded as running out of memory (a failure std recovers from, `try_reserve`, is followed by a success that
//! clears it). With the `alloc-count` feature each thread's allocations are counted, for the tests that prove a path
//! allocates nothing.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

/// Declared by the daemon as its `#[global_allocator]`.
pub struct Alloc;

thread_local! {
    // const, without a destructor: reading or writing it never allocates, even while the thread exits
    static LAST_FAILED: Cell<bool> = const { Cell::new(false) };
}

fn failed(ptr: *mut u8) -> *mut u8 {
    LAST_FAILED.with(|f| f.set(ptr.is_null()));
    ptr
}

/// This thread's last allocation failed.
pub fn allocation_failed() -> bool {
    LAST_FAILED.with(Cell::get)
}

#[cfg(feature = "alloc-count")]
thread_local! {
    // const, without a destructor: reading it never allocates
    static ALLOCATIONS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[cfg(feature = "alloc-count")]
fn count() {
    ALLOCATIONS.with(|a| a.set(a.get() + 1));
}

#[cfg(not(feature = "alloc-count"))]
fn count() {}

/// The allocations this thread made so far (`alloc-count` builds).
#[cfg(feature = "alloc-count")]
pub fn allocations() -> u64 {
    ALLOCATIONS.with(std::cell::Cell::get)
}

// SAFETY: every method passes the caller's pointer and layout to `System` unchanged, so `System`'s guarantees are
// the allocator's; the counter and the callback touch no memory the allocation returns.
unsafe impl GlobalAlloc for Alloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        // SAFETY: the caller's `layout` contract is `System::alloc`'s.
        failed(unsafe { System.alloc(layout) })
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count();
        // SAFETY: as `alloc`.
        failed(unsafe { System.alloc_zeroed(layout) })
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from this allocator, which is `System`, with this `layout`.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count();
        // SAFETY: `ptr` and `layout` came from `System` through this allocator; `new_size` is the caller's contract.
        failed(unsafe { System.realloc(ptr, layout, new_size) })
    }
}
