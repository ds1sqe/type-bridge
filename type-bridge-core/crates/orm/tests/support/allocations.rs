//! Count allocations of one known future layout on the current test thread.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::future::Future;

thread_local! {
    static TARGET: Cell<usize> = const { Cell::new(0) };
    static COUNT: Cell<usize> = const { Cell::new(0) };
}
struct CountingAllocator;
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn observe(bytes: usize) {
    let _ = TARGET.try_with(|target| {
        if target.get() != 0 && target.get() == bytes {
            let _ = COUNT.try_with(|count| count.set(count.get() + 1));
        }
    });
}
// Safety: every operation forwards the unchanged layout/pointer to System.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        observe(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        observe(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        observe(size);
        unsafe { System.realloc(ptr, layout, size) }
    }
}
struct Scope;
impl Drop for Scope {
    fn drop(&mut self) {
        TARGET.set(0);
    }
}
pub async fn count<F: Future>(bytes: usize, future: F) -> (F::Output, usize) {
    assert!(bytes > 0);
    assert_eq!(TARGET.get(), 0);
    COUNT.set(0);
    TARGET.set(bytes);
    let scope = Scope;
    let result = future.await;
    drop(scope);
    (result, COUNT.get())
}
