//! Windows-specific memory benchmarks that try explore different memory allocation strategies
//! using built-in Windows mechanisms under different operating conditions.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use criterion::{criterion_group, criterion_main};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

#[cfg(not(windows))]
use not_windows::entrypoint;
#[cfg(windows)]
use windows::entrypoint;

#[cfg(not(windows))]
mod not_windows {
    use criterion::Criterion;

    pub(crate) fn entrypoint(_c: &mut Criterion) {}
}

#[cfg(windows)]
mod windows {
    use std::{
        cell::RefCell,
        hint::black_box,
        iter::repeat_with,
        sync::{Arc, Mutex},
    };

    use criterion::Criterion;
    use many_cpus::{HardwareInfo, HardwareTracker};
    use many_cpus_benchmarking::{Payload, WorkDistribution, execute_runs};
    use windows::Win32::{
        Foundation::HANDLE,
        System::Memory::{
            GetProcessHeap, HEAP_FLAGS, HEAP_NO_SERIALIZE, HeapAlloc, HeapCreate, HeapDestroy,
            HeapFree,
        },
    };

    // https://github.com/cloudhead/nonempty/issues/68
    extern crate alloc;

    pub(crate) fn entrypoint(c: &mut Criterion) {
        execute_runs::<AllocDefaultHeap, 1000>(c, WorkDistribution::all());
        execute_runs::<AllocPerThreadHeap, 1000>(c, WorkDistribution::all());
        execute_runs::<AllocPerThreadHeapThreadSafe, 1000>(c, WorkDistribution::all());
        execute_runs::<AllocPerMemoryRegionHeap, 1000>(c, WorkDistribution::all());
    }

    const CHUNK_SIZE: usize = 1024; // 1 KB
    const CHUNK_COUNT: usize = 1024 * 100; // -> 100 MB

    // The expectation here is that the default heap will quickly grow to be a large size to fit
    // all our data, so to make a fair comparison we might as well start off all the heaps at the
    // right size (because the non-defaults are (re)created multiple times in benchmarking).
    // We double the size because the heap may need some padding or have overhead - not every
    // item can be allocated next to each other (presumably).
    const HEAP_INITIAL_SIZE: usize = CHUNK_SIZE * CHUNK_COUNT * 2;

    /// Allocates and frees memory on the default Windows heap assigned to the process.
    #[derive(Debug)]
    struct AllocDefaultHeap {
        process_heap: HANDLE,

        chunk_buffer: Vec<*mut std::ffi::c_void>,
    }

    impl AllocDefaultHeap {
        fn new() -> Self {
            Self {
                process_heap: HANDLE::default(),
                chunk_buffer: Vec::with_capacity(CHUNK_COUNT),
            }
        }
    }

    impl Payload for AllocDefaultHeap {
        fn new_pair() -> (Self, Self) {
            (Self::new(), Self::new())
        }

        fn prepare(&mut self) {
            // SAFETY: No safety requirements. We do not need to free this handle - it is virtual.
            self.process_heap = unsafe { GetProcessHeap() }.unwrap();
        }

        fn process(&mut self) {
            self.chunk_buffer.extend(
                // SAFETY: No safety requirements.
                repeat_with(|| unsafe { HeapAlloc(self.process_heap, HEAP_FLAGS(0), CHUNK_SIZE) })
                    .take(CHUNK_COUNT),
            );

            for chunk in self.chunk_buffer.drain(..) {
                // SAFETY: No safety requirements.
                unsafe { HeapFree(self.process_heap, HEAP_FLAGS(0), Some(chunk)) }.unwrap();
            }
        }
    }

    // SAFETY: It just complains because of the HANDLE and pointers - in reality, fine to send them.
    unsafe impl Send for AllocDefaultHeap {}

    /// Allocates and frees memory on the a custom per-thread non-thread-safe Windows heap.
    #[derive(Debug)]
    struct AllocPerThreadHeap {
        chunk_buffer: Vec<*mut std::ffi::c_void>,
    }

    impl AllocPerThreadHeap {
        fn new() -> Self {
            Self {
                chunk_buffer: Vec::with_capacity(CHUNK_COUNT),
            }
        }
    }

    impl Payload for AllocPerThreadHeap {
        fn new_pair() -> (Self, Self) {
            (Self::new(), Self::new())
        }

        fn prepare_local(&mut self) {
            // Initialize the heap before the measurement starts.
            PER_THREAD_HEAP.with(|heap| _ = black_box(heap));
        }

        fn process(&mut self) {
            PER_THREAD_HEAP.with(|heap| {
                self.chunk_buffer
                    .extend(repeat_with(|| heap.alloc(CHUNK_SIZE)).take(CHUNK_COUNT));

                for chunk in self.chunk_buffer.drain(..) {
                    // SAFETY: The pointee must be allocated on this heap. It is.
                    unsafe {
                        heap.free(chunk);
                    }
                }
            });
        }
    }

    // SAFETY: It just complains due to the pointers - that's fine, all is well.
    unsafe impl Send for AllocPerThreadHeap {}

    /// Allocates and frees memory on the a custom per-thread thread-safe Windows heap.
    #[derive(Debug)]
    struct AllocPerThreadHeapThreadSafe {
        chunk_buffer: Vec<*mut std::ffi::c_void>,
    }

    impl AllocPerThreadHeapThreadSafe {
        fn new() -> Self {
            Self {
                chunk_buffer: Vec::with_capacity(CHUNK_COUNT),
            }
        }
    }

    impl Payload for AllocPerThreadHeapThreadSafe {
        fn new_pair() -> (Self, Self) {
            (Self::new(), Self::new())
        }

        fn prepare_local(&mut self) {
            // Initialize the heap before the measurement starts.
            PER_THREAD_HEAP_THREAD_SAFE.with(|heap| _ = black_box(heap));
        }

        fn process(&mut self) {
            PER_THREAD_HEAP_THREAD_SAFE.with(|heap| {
                self.chunk_buffer
                    .extend(repeat_with(|| heap.alloc(CHUNK_SIZE)).take(CHUNK_COUNT));

                for chunk in self.chunk_buffer.drain(..) {
                    // SAFETY: The pointee must be allocated on this heap. It is.
                    unsafe {
                        heap.free(chunk);
                    }
                }
            });
        }
    }

    // SAFETY: It just complains due to the pointers - that's fine, all is well.
    unsafe impl Send for AllocPerThreadHeapThreadSafe {}

    /// Allocates and frees memory on the a custom per-thread thread-safe Windows heap.
    #[derive(Debug)]
    struct AllocPerMemoryRegionHeap {
        chunk_buffer: Vec<*mut std::ffi::c_void>,
    }

    impl AllocPerMemoryRegionHeap {
        fn new() -> Self {
            Self {
                chunk_buffer: Vec::with_capacity(CHUNK_COUNT),
            }
        }
    }

    impl Payload for AllocPerMemoryRegionHeap {
        fn new_pair() -> (Self, Self) {
            (Self::new(), Self::new())
        }

        fn prepare_local(&mut self) {
            // Initialize the heap before the measurement starts.
            PER_REGION_HEAP.with(|heap| _ = black_box(heap));
        }

        fn process(&mut self) {
            PER_REGION_HEAP.with(|heap| {
                self.chunk_buffer
                    .extend(repeat_with(|| heap.alloc(CHUNK_SIZE)).take(CHUNK_COUNT));

                for chunk in self.chunk_buffer.drain(..) {
                    heap.free(chunk);
                }
            });
        }
    }

    // SAFETY: It just complains due to the pointers - that's fine, all is well.
    unsafe impl Send for AllocPerMemoryRegionHeap {}

    thread_local!(static PER_THREAD_HEAP_THREAD_SAFE: ThreadSafeCustomHeap = ThreadSafeCustomHeap::new());
    thread_local!(static PER_THREAD_HEAP: SingleThreadedCustomHeap = SingleThreadedCustomHeap::new());
    linked::thread_local_rc!(static PER_REGION_HEAP: MemoryRegionSpecificHeap = MemoryRegionSpecificHeap::new());

    struct SingleThreadedCustomHeap {
        heap: HANDLE,
    }

    impl SingleThreadedCustomHeap {
        fn new() -> Self {
            Self {
                // SAFETY: We fulfill the requirement to destroy this handle in drop().
                heap: unsafe { HeapCreate(HEAP_NO_SERIALIZE, HEAP_INITIAL_SIZE, 0) }.unwrap(),
            }
        }

        fn alloc(&self, size: usize) -> *mut std::ffi::c_void {
            // SAFETY: No safety requirements.
            unsafe { HeapAlloc(self.heap, HEAP_NO_SERIALIZE, size) }
        }

        /// # Safety
        ///
        /// The caller must ensure that the pointer was allocated by this heap.
        unsafe fn free(&self, ptr: *mut std::ffi::c_void) {
            // SAFETY: Forwarding "same heap" requirement to the caller.
            unsafe { HeapFree(self.heap, HEAP_NO_SERIALIZE, Some(ptr)) }.unwrap();
        }
    }

    impl Drop for SingleThreadedCustomHeap {
        fn drop(&mut self) {
            // SAFETY: No safety requirements.
            unsafe { HeapDestroy(self.heap) }.unwrap();
        }
    }

    struct ThreadSafeCustomHeap {
        heap: HANDLE,
    }

    impl ThreadSafeCustomHeap {
        fn new() -> Self {
            Self {
                // SAFETY: No safety requirements.
                heap: unsafe { HeapCreate(HEAP_FLAGS(0), HEAP_INITIAL_SIZE, 0) }.unwrap(),
            }
        }

        fn alloc(&self, size: usize) -> *mut std::ffi::c_void {
            // SAFETY: No safety requirements.
            unsafe { HeapAlloc(self.heap, HEAP_FLAGS(0), size) }
        }

        /// # Safety
        ///
        /// The caller must ensure that the pointer was allocated by this heap.
        unsafe fn free(&self, ptr: *mut std::ffi::c_void) {
            // SAFETY: Forwarding "same heap" requirement to the caller.
            unsafe { HeapFree(self.heap, HEAP_FLAGS(0), Some(ptr)) }.unwrap();
        }
    }

    impl Drop for ThreadSafeCustomHeap {
        fn drop(&mut self) {
            // SAFETY: No safety requirements.
            unsafe { HeapDestroy(self.heap) }.unwrap();
        }
    }

    // SAFETY: The HANDLE is not thread-safe by type but is logically thread-safe in this case.
    unsafe impl Send for ThreadSafeCustomHeap {}
    // SAFETY: The HANDLE is not thread-safe by type but is logically thread-safe in this case.
    unsafe impl Sync for ThreadSafeCustomHeap {}

    #[linked::object]
    struct MemoryRegionSpecificHeap {
        // Assigned when the thread first tries to allocate region-specific memory.
        // We assume that the same thread will never move to another region in this case,
        // otherwise, we will be sharing the heap across regions, which is not what we want.
        current_region_heap: RefCell<Option<Arc<ThreadSafeCustomHeap>>>,

        // This is our big bag of all the heaps. We create the heap on-demand,
        // on a thread that is running in the target memory region.
        #[expect(
            clippy::type_complexity,
            reason = "will allow since it is only used once"
        )]
        per_region_heaps: Arc<Mutex<Box<[Option<Arc<ThreadSafeCustomHeap>>]>>>,
    }

    impl MemoryRegionSpecificHeap {
        fn new() -> Self {
            let memory_region_count = HardwareInfo::max_memory_region_count();
            let per_region_heaps = Arc::new(Mutex::new(
                vec![None; memory_region_count].into_boxed_slice(),
            ));

            linked::new!(Self {
                current_region_heap: RefCell::new(None),
                per_region_heaps: Arc::clone(&per_region_heaps),
            })
        }

        fn alloc(&self, size: usize) -> *mut std::ffi::c_void {
            self.with_current_heap(|heap| heap.alloc(size))
        }

        /// # Safety
        ///
        /// The caller must ensure that the pointer was allocated by this heap.
        fn free(&self, ptr: *mut std::ffi::c_void) {
            // SAFETY: Forwarding "same heap" requirement to the caller.
            self.with_current_heap(|heap| unsafe { heap.free(ptr) });
        }

        fn with_current_heap<F, R>(&self, f: F) -> R
        where
            F: FnOnce(&ThreadSafeCustomHeap) -> R,
        {
            let mut current_region_heap = self.current_region_heap.borrow_mut();

            let heap = current_region_heap.get_or_insert_with(|| {
                let region = HardwareTracker::current_memory_region_id();

                let mut heaps = self.per_region_heaps.lock().unwrap();

                let heap = heaps.get_mut(region as usize)
                    .expect("current memory region ID is out of bounds - the platform lied about how many there are")
                    .get_or_insert_with(|| Arc::new(ThreadSafeCustomHeap::new()));

                Arc::clone(heap)
            });

            f(heap)
        }
    }
}
