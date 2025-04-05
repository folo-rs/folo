//! Demonstrates basic usage of the benchmark harness provided by `many_cpus_benchmarking`.

#![allow(missing_docs)] // No need for API documentation in benchmark code.

use std::{hint::black_box, ptr};

use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus_benchmarking::{Payload, WorkDistribution, execute_runs};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    // We use a BATCH_SIZE of 10, which means 10 * 64 = 640 MB of memory used per worker pair.
    execute_runs::<CopyBytes, 10>(c, WorkDistribution::all());
}

const COPY_BYTES_LEN: usize = 64 * 1024 * 1024;

/// Sample benchmark scenario that copies bytes between the two paired payloads.
///
/// The source buffers are allocated in the "prepare" step and become local to the "prepare" worker.
/// The destination buffers are allocated in the "process" step. The end result is that we copy
/// from remote memory (allocated in the "prepare" step) to local memory in the "process" step.
///
/// There is no deep meaning behind this scenario, just a sample benchmark that showcases comparing
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

        // SAFETY: We just filled these bytes, it is all good.
        unsafe {
            to.set_len(COPY_BYTES_LEN);
        }

        // Read from the destination to prevent the compiler from optimizing the copy away.
        _ = black_box(to[0]);
    }
}
