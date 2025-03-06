use std::ptr;

use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus_benchmarking::{Payload, WorkDistribution, execute_runs};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    execute_runs::<CopyBytes, 1>(c, WorkDistribution::all());
}

const COPY_BYTES_LEN: usize = 64 * 1024 * 1024;

/// Sample benchmark scenario that copies bytes between the two paired payloads.
///
/// The source buffers are allocated in the "prepare" step to make them local to each worker.
/// The destination buffers are allocated in the "process" step. The end result is that we copy
/// from remote memory to local memory (and also perform a local allocation).
///
/// There is no deep meaning behind this, just a sample benchmark that showcases comparing
/// different work distribution modes to identify performance differences from hardware-awareness.
#[derive(Debug, Default)]
struct CopyBytes {
    from: Option<Vec<u8>>,
}

impl Payload for CopyBytes {
    fn new_pair() -> (Self, Self) {
        (Self::default(), Self::default())
    }

    fn prepare(&mut self) {
        self.from = Some(vec![99; COPY_BYTES_LEN]);
    }

    fn process(&mut self) {
        let from = self.from.as_ref().unwrap();
        let mut to = Vec::with_capacity(COPY_BYTES_LEN);

        // SAFETY: The pointers are valid, the length is correct, all is well.
        unsafe {
            ptr::copy_nonoverlapping(from.as_ptr(), to.as_mut_ptr(), COPY_BYTES_LEN);
        }
    }
}
