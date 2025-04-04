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

    pub fn entrypoint(_c: &mut Criterion) {}
}

#[cfg(windows)]
mod windows {
    use std::{
        cell::RefCell,
        hint::black_box,
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

    pub fn entrypoint(c: &mut Criterion) {
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
        pub fn new() -> Self {
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
            self.process_heap = unsafe { GetProcessHeap() }.unwrap();
        }

        fn process(&mut self) {
            self.chunk_buffer.extend(
                (0..CHUNK_COUNT)
                    .map(|_| unsafe { HeapAlloc(self.process_heap, HEAP_FLAGS(0), CHUNK_SIZE) }),
            );

            for chunk in self.chunk_buffer.drain(..) {
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
        pub fn new() -> Self {
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
                    .extend((0..CHUNK_COUNT).map(|_| heap.alloc(CHUNK_SIZE)));

                for chunk in self.chunk_buffer.drain(..) {
                    heap.free(chunk);
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
        pub fn new() -> Self {
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
                    .extend((0..CHUNK_COUNT).map(|_| heap.alloc(CHUNK_SIZE)));

                for chunk in self.chunk_buffer.drain(..) {
                    heap.free(chunk);
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
        pub fn new() -> Self {
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
                    .extend((0..CHUNK_COUNT).map(|_| heap.alloc(CHUNK_SIZE)));

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
    linked::instance_per_thread!(static PER_REGION_HEAP: MemoryRegionSpecificHeap = MemoryRegionSpecificHeap::new());

    struct SingleThreadedCustomHeap {
        heap: HANDLE,
    }

    impl SingleThreadedCustomHeap {
        pub fn new() -> Self {
            Self {
                heap: unsafe { HeapCreate(HEAP_NO_SERIALIZE, HEAP_INITIAL_SIZE, 0) }.unwrap(),
            }
        }

        pub fn alloc(&self, size: usize) -> *mut std::ffi::c_void {
            unsafe { HeapAlloc(self.heap, HEAP_NO_SERIALIZE, size) }
        }

        pub fn free(&self, ptr: *mut std::ffi::c_void) {
            unsafe { HeapFree(self.heap, HEAP_NO_SERIALIZE, Some(ptr)) }.unwrap();
        }
    }

    impl Drop for SingleThreadedCustomHeap {
        fn drop(&mut self) {
            unsafe { HeapDestroy(self.heap) }.unwrap();
        }
    }

    struct ThreadSafeCustomHeap {
        heap: HANDLE,
    }

    impl ThreadSafeCustomHeap {
        pub fn new() -> Self {
            Self {
                heap: unsafe { HeapCreate(HEAP_FLAGS(0), HEAP_INITIAL_SIZE, 0) }.unwrap(),
            }
        }

        pub fn alloc(&self, size: usize) -> *mut std::ffi::c_void {
            unsafe { HeapAlloc(self.heap, HEAP_FLAGS(0), size) }
        }

        pub fn free(&self, ptr: *mut std::ffi::c_void) {
            unsafe { HeapFree(self.heap, HEAP_FLAGS(0), Some(ptr)) }.unwrap();
        }
    }

    impl Drop for ThreadSafeCustomHeap {
        fn drop(&mut self) {
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
        #[allow(clippy::type_complexity)]
        per_region_heaps: Arc<Mutex<Box<[Option<Arc<ThreadSafeCustomHeap>>]>>>,
    }

    impl MemoryRegionSpecificHeap {
        pub fn new() -> Self {
            let memory_region_count = HardwareInfo::max_memory_region_count();
            let per_region_heaps = Arc::new(Mutex::new(
                vec![None; memory_region_count].into_boxed_slice(),
            ));

            linked::new!(Self {
                current_region_heap: RefCell::new(None),
                per_region_heaps: Arc::clone(&per_region_heaps),
            })
        }

        pub fn alloc(&self, size: usize) -> *mut std::ffi::c_void {
            self.with_current_heap(|heap| heap.alloc(size))
        }

        pub fn free(&self, ptr: *mut std::ffi::c_void) {
            self.with_current_heap(|heap| heap.free(ptr))
        }

        fn with_current_heap<F, R>(&self, f: F) -> R
        where
            F: FnOnce(&ThreadSafeCustomHeap) -> R,
        {
            let mut current_region_heap = self.current_region_heap.borrow_mut();

            let heap = current_region_heap.get_or_insert_with(|| {
                let region = HardwareTracker::current_memory_region_id();

                let mut heaps = self.per_region_heaps.lock().unwrap();

                let heap = heaps[region as usize]
                    .get_or_insert_with(|| Arc::new(ThreadSafeCustomHeap::new()));

                Arc::clone(heap)
            });

            f(heap)
        }
    }
}
