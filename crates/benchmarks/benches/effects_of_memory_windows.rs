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

    pub fn entrypoint(c: &mut Criterion) {}
}

mod windows {
    use criterion::Criterion;
    use itertools::Itertools;
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
        execute_runs::<AllocDefaultHeap, 10_000>(c, WorkDistribution::all());
        execute_runs::<AllocPerThreadHeap, 10_000>(c, WorkDistribution::all());
        execute_runs::<AllocPerThreadHeapThreadSafe, 10_000>(c, WorkDistribution::all());
    }

    const CHUNK_SIZE: usize = 1024; // 1 KB
    const CHUNK_COUNT: usize = 1024 * 100; // -> 100 MB

    /// Allocates and frees memory on the default Windows heap assigned to the process.
    #[derive(Debug, Default)]
    struct AllocDefaultHeap {
        process_heap: HANDLE,
    }

    impl Payload for AllocDefaultHeap {
        fn new_pair() -> (Self, Self) {
            (Self::default(), Self::default())
        }

        fn prepare(&mut self) {
            self.process_heap = unsafe { GetProcessHeap() }.unwrap();
        }

        fn process(&mut self) {
            let chunks = (0..CHUNK_COUNT)
                .map(|_| unsafe { HeapAlloc(self.process_heap, HEAP_FLAGS(0), CHUNK_SIZE) })
                .collect_vec();

            for chunk in chunks {
                unsafe { HeapFree(self.process_heap, HEAP_FLAGS(0), Some(chunk)) }.unwrap();
            }
        }
    }

    // SAFETY: It just complains because of the HANDLE - in reality, fine to send it.
    unsafe impl Send for AllocDefaultHeap {}

    /// Allocates and frees memory on the a custom per-thread non-thread-safe Windows heap.
    #[derive(Debug, Default)]
    struct AllocPerThreadHeap {}

    impl Payload for AllocPerThreadHeap {
        fn new_pair() -> (Self, Self) {
            (Self::default(), Self::default())
        }

        fn prepare(&mut self) {}

        fn process(&mut self) {
            PER_THREAD_HEAP.with(|heap| {
                let chunks = (0..CHUNK_COUNT)
                    .map(|_| heap.alloc(CHUNK_SIZE))
                    .collect_vec();

                for chunk in chunks {
                    heap.free(chunk);
                }
            });
        }
    }

    /// Allocates and frees memory on the a custom per-thread thread-safe Windows heap.
    #[derive(Debug, Default)]
    struct AllocPerThreadHeapThreadSafe {}

    impl Payload for AllocPerThreadHeapThreadSafe {
        fn new_pair() -> (Self, Self) {
            (Self::default(), Self::default())
        }

        fn prepare(&mut self) {}

        fn process(&mut self) {
            PER_THREAD_HEAP_THREAD_SAFE.with(|heap| {
                let chunks = (0..CHUNK_COUNT)
                    .map(|_| heap.alloc(CHUNK_SIZE))
                    .collect_vec();

                for chunk in chunks {
                    heap.free(chunk);
                }
            });
        }
    }

    thread_local!(static PER_THREAD_HEAP_THREAD_SAFE: CustomHeap = CustomHeap::new(true));
    thread_local!(static PER_THREAD_HEAP: CustomHeap = CustomHeap::new(false));

    struct CustomHeap {
        heap: HANDLE,
    }

    impl CustomHeap {
        pub fn new(thread_safe: bool) -> Self {
            let flags = if thread_safe {
                HEAP_FLAGS(0)
            } else {
                HEAP_NO_SERIALIZE
            };

            Self {
                heap: unsafe { HeapCreate(flags, 0, 0) }.unwrap(),
            }
        }

        pub fn alloc(&self, size: usize) -> *mut std::ffi::c_void {
            unsafe { HeapAlloc(self.heap, HEAP_FLAGS(0), size) }
        }

        pub fn free(&self, ptr: *mut std::ffi::c_void) {
            unsafe { HeapFree(self.heap, HEAP_FLAGS(0), Some(ptr)) }.unwrap();
        }
    }

    impl Drop for CustomHeap {
        fn drop(&mut self) {
            unsafe { HeapDestroy(self.heap) }.unwrap();
        }
    }
}
