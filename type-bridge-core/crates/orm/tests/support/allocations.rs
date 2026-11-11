//! Count allocations of one known future layout on the current test thread.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::future::Future;

thread_local! {
    static TARGET: Cell<usize> = const { Cell::new(0) };
    static ORIGIN: Cell<Option<&'static str>> = const { Cell::new(None) };
    static COUNT: Cell<usize> = const { Cell::new(0) };
}
struct CountingAllocator;
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn observe(bytes: usize) {
    let _ = TARGET.try_with(|target| {
        if target.get() != 0 && target.get() == bytes {
            let matches = ORIGIN.with(|origin| match origin.get() {
                None => true,
                Some(origin) => {
                    // Symbolization allocates; exclude that work from the probe.
                    target.set(0);
                    let trace = std::backtrace::Backtrace::force_capture().to_string();
                    target.set(bytes);
                    trace.contains(origin)
                }
            });
            if matches {
                let _ = COUNT.try_with(|count| count.set(count.get() + 1));
            }
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
        ORIGIN.set(None);
    }
}
pub async fn count<F: Future>(bytes: usize, future: F) -> (F::Output, usize) {
    count_from(bytes, None, future).await
}

/// Count matching allocations, optionally only those whose backtrace names
/// `origin`.
///
/// Origin filtering needs a backtrace captured inside the global allocator
/// to unwind past the allocator shim into the caller. Linux and Windows CI
/// do; on the macOS runner no caller frame is symbolized (the positive control
/// counted zero in CI), which would make an origin-filtered zero vacuous.
pub async fn count_from<F: Future>(
    bytes: usize,
    origin: Option<&'static str>,
    future: F,
) -> (F::Output, usize) {
    assert!(bytes > 0);
    assert!(
        origin.is_none() || !cfg!(target_os = "macos"),
        "allocation origin filtering is unreliable on macOS"
    );
    assert_eq!(TARGET.get(), 0);
    COUNT.set(0);
    ORIGIN.set(origin);
    TARGET.set(bytes);
    let scope = Scope;
    let result = future.await;
    drop(scope);
    (result, COUNT.get())
}

#[cfg(not(target_os = "macos"))]
#[inline(never)]
fn allocation_origin_probe(bytes: usize) -> Box<[u8]> {
    vec![0; bytes].into_boxed_slice()
}

#[cfg(not(target_os = "macos"))]
#[tokio::test]
async fn allocation_origin_filter_keeps_matching_allocations() {
    for (origin, expected) in [
        ("allocations::allocation_origin_probe", 1),
        ("allocations::unrelated_function", 0),
    ] {
        let (value, count) = count_from(632, Some(origin), async {
            allocation_origin_probe(std::hint::black_box(632))
        })
        .await;
        assert_eq!(value.len(), 632);
        assert_eq!(count, expected);
    }
}
