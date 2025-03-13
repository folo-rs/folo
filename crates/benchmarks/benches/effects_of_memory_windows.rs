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
        System::Memory::{GetProcessHeap, HEAP_FLAGS, HeapAlloc, HeapFree},
    };

    // https://github.com/cloudhead/nonempty/issues/68
    extern crate alloc;

    pub fn entrypoint(c: &mut Criterion) {
        execute_runs::<AllocDefaultHeap, 10_000>(c, WorkDistribution::all());
    }

    const CHUNK_SIZE: usize = 10 * 1024; // 10 KB
    const CHUNK_COUNT: usize = 1024 * 100; // x 100 = 1 MB x 1024 = 1 GB

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
}
