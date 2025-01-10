use std::{
    mem,
    num::NonZeroUsize,
    sync::{mpsc, Arc, Barrier, Mutex},
    thread::JoinHandle,
};

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use folo_hw::ProcessorSet;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

const TWO_PROCESSORS: NonZeroUsize = NonZeroUsize::new(2).unwrap();

fn entrypoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("cross_memory_region_channels");

    let far_processor_pair = ProcessorSet::builder()
        .performance_processors_only()
        .different_memory_regions()
        .take(TWO_PROCESSORS)
        .expect("must have two processors in different memory regions for this benchmark");

    let near_processor_pair = ProcessorSet::builder()
        .performance_processors_only()
        .same_memory_region()
        .take(TWO_PROCESSORS)
        .expect("must have two processors in the same memory region for this benchmark");

    group.bench_function("same_region_channels", |b| {
        b.iter_batched(
            || BenchmarkRun::new(&near_processor_pair),
            |run| run.wait(),
            BatchSize::PerIteration,
        );
    });

    group.bench_function("different_region_channels", |b| {
        b.iter_batched(
            || BenchmarkRun::new(&far_processor_pair),
            |run| run.wait(),
            BatchSize::PerIteration,
        );
    });

    group.finish();
}

// The benchmark is preloaded with this much data.
const PAYLOAD_COUNT: u64 = 1_000_000;

// We execute this many iterations on each worker thread, each moving one payload.
const WORKER_ITERATION_COUNT: usize = 100_000;

type Payload = u64;

// TODO: Avoid hanging forever is barrier is not released.

struct BenchmarkRun {
    // Waited 3 times, once be each worker thread and once by runner thread.
    completed: Arc<Barrier>,

    join_handles: Box<[JoinHandle<()>]>,
}

impl BenchmarkRun {
    fn new(processor_pair: &ProcessorSet) -> Self {
        assert_eq!(processor_pair.len(), 2);

        // We create two channels, one for processor A->B, one for B->A.
        // We pre-fill the both channels with a bunch of data to ensure threads have lots of work.
        // When the threads start, they will simply start passing messages to each other.
        // When enough messages have been passed, the run is marked completed.
        let (c1_tx, c1_rx) = mpsc::channel::<Payload>();
        let (c2_tx, c2_rx) = mpsc::channel::<Payload>();

        for i in 0..PAYLOAD_COUNT {
            c1_tx.send(i).unwrap();
            c2_tx.send(i).unwrap();
        }

        // Once by current thread, once by each worker.
        let ready = Arc::new(Barrier::new(3));

        // We stuff them in a bag. Whichever thread starts first takes the first group.
        let bag = Arc::new(Mutex::new(vec![
            (c1_tx, c1_rx, Arc::clone(&ready)),
            (c2_tx, c2_rx, Arc::clone(&ready)),
        ]));

        let completed = Arc::new(Barrier::new(3));

        // TODO: Should this be using an entrypoint factory? What if input is not cloneable?
        let join_handles = processor_pair.spawn_threads({
            let completed = Arc::clone(&completed);

            move |_| {
                let bag = Arc::clone(&bag);
                let completed = Arc::clone(&completed);

                let (tx, rx, ready) = bag.lock().unwrap().pop().unwrap();

                ready.wait();

                for _ in 0..WORKER_ITERATION_COUNT {
                    let payload = rx.recv().unwrap();
                    tx.send(payload).unwrap();
                }

                completed.wait();
            }
        });

        ready.wait();

        Self {
            completed,
            join_handles,
        }
    }

    fn wait(&self) {
        self.completed.wait();
    }
}

impl Drop for BenchmarkRun {
    fn drop(&mut self) {
        let join_handles = mem::replace(&mut self.join_handles, Box::new([]));

        for handle in join_handles {
            handle.join().unwrap();
        }
    }
}
