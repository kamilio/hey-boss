//! Thread-local allocation measurements; other tests remain unobserved.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static REQUESTED: Cell<Option<usize>> = const { Cell::new(None) };
}

struct Allocator;
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

fn record(bytes: usize) {
    let _ = REQUESTED.try_with(|count| {
        if let Some(total) = count.get() {
            count.set(Some(total.saturating_add(bytes)));
        }
    });
}

// The wrapper delegates every operation to the standard allocator. Accounting
// is disabled by default and never allocates or observes another test thread.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record(size);
        unsafe { System.realloc(ptr, layout, size) }
    }
}

pub(crate) fn measure<T>(operation: impl FnOnce() -> T) -> (T, usize) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            REQUESTED.with(|count| count.set(None));
        }
    }
    REQUESTED.with(|count| assert!(count.replace(Some(0)).is_none()));
    let reset = Reset;
    let result = operation();
    let bytes = REQUESTED.with(|count| count.get().unwrap());
    drop(reset);
    (result, bytes)
}
