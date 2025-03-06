# many_cpus_benchmarking

Criterion benchmark harness designed to compare different modes of distributing work in a many-
processor system with multiple memory regions. This helps highlight the performance impact of
cross-memory-region data transfers, cross-processor data transfers and multi-threaded logic.

# Execution model

The benchmark harness selects **pairs of processors** that will execute multiple iterations of a
benchmark scenario, preparing and processing **payloads**. The end result is the total duration
spent by each pair (counting whichever pair member takes longest to process the payload).

The criteria for processor pairs selection is determined by the specified [`WorkDistribution`],
with the final selection randomized for each iteration if there are multiple equally valid candidate
processor pairs.

# Usage

For each benchmark scenario, define a type that implements the [`Payload`] trait. Executing a
benchmark scenario consists will undergo the following major steps:

1. For each processor pair, [a payload pair is created][3].
1. Each payload is moved to its assigned processor and [prepared][4]. This is where the payload data
   set is typically generated.
1. Depending on the work distribution mode, the payloads may now be exchanged between the assigned
   processors, to ensure that we process "foreign" data on each processor.
1. The payload is [processed][5] by each worker in the pair. This is the timed step.

The reference to "foreign" data here implies that if the two workers are in different memory
regions, the data is likely to be present in a different memory region than used by the processor
used to process the payload.

This is because physical memory pages are allocated in the memory region that initializes any heap-
allocated memory (in the "prepare" step), so despite the payload later being moved to a different
worker's thread, any heap-allocated data referenced by the payload remains where it is, potentially
in physical memory modules that are not directly connected to the current processor.

# Example

A simple scenario that merely copies memory from a foreign buffer to a local one
(`benches/many_cpus_harness_demo.rs`):

```rust ignore (benchmark)
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
```

This scenario is executed in a Criterion benchmark by calling [`execute_runs()`][6] and providing
the desired work distribution modes to use:

```rust ignore (benchmark)
fn entrypoint(c: &mut Criterion) {
    execute_runs::<CopyBytes, 1>(c, WorkDistribution::all());
}
```

Example output (in `target/criterion/report` after benchmarking):

<img src="https://media.githubusercontent.com/media/folo-rs/folo/refs/heads/main/crates/many_cpus_benchmarking/images/work_distribution_comparison.png">

# Payload multiplier

It may sometimes be desirable to multiply the size of a benchmark scenario, e.g. if a scenario is
very fast and completes too quickly for meaningful or comparable measurements due to the
worker orchestration overhead.

Use the second generic parameter of `execute_runs` to apply a multiplier to the payload size. This
simply uses multiple payloads for each iteration (on the same worker), allowing the impact from the
benchmark harness overheads to be reduced, so the majority of the time is spent on payload
processing.

[3]: crate::Payload::new_pair
[4]: crate::Payload::prepare
[5]: crate::Payload::process
[6]: crate::execute_runs