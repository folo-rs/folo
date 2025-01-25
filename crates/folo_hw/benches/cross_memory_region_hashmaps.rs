use std::{
    collections::HashMap,
    hint::black_box,
    mem,
    num::NonZeroUsize,
    sync::{mpsc, Arc, Barrier, LazyLock, Mutex},
    thread::JoinHandle,
    time::Duration,
};

use criterion::{criterion_group, criterion_main, BatchSize, Criterion, SamplingMode};
use folo_hw::ProcessorSet;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

const TWO_PROCESSORS: NonZeroUsize = NonZeroUsize::new(2).unwrap();

fn entrypoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("cross_memory_region_hashmaps");

    // This stuff takes forever, so be patient.
    //group.sample_size(10);
    //group.sampling_mode(SamplingMode::Flat);
    //group.warm_up_time(Duration::from_secs(10));
    group.measurement_time(Duration::from_secs(120));

    if let Some(far_processor_pair) = ProcessorSet::builder()
        .performance_processors_only()
        .different_memory_regions()
        .take(TWO_PROCESSORS)
    {
        group.bench_function("different_memory_region", |b| {
            b.iter_batched(
                || BenchmarkRun::new(&far_processor_pair, RunConfiguration::DifferentMemoryRegion),
                |run| run.wait(),
                BatchSize::PerIteration,
            );
        });
    } else {
        eprintln!(
            "skipping different_memory_region benchmark because the PC only has one memory region"
        );
    }

    if let Some(near_processor_pair) = ProcessorSet::builder()
        .performance_processors_only()
        .same_memory_region()
        .take(TWO_PROCESSORS)
    {
        group.bench_function("same_memory_region", |b| {
            b.iter_batched(
                || BenchmarkRun::new(&near_processor_pair, RunConfiguration::SameMemoryRegion),
                |run| run.wait(),
                BatchSize::PerIteration,
            );
        });
    } else {
        eprintln!("skipping same_memory_region benchmark because the PC has no memory region with two available processors");
    }

    group.finish();
}

type Payload = HashMap<u64, u64>;

// The size of this has some interplay with CPU cache sizes etc, with different cache effects to be
// expected on different hardware. We anyway expect the critical parts of the data structure to end
// up in L3 cache (there is gigabytes of it!) so a large size here is unlikely to benefit us much
// in terms of allowing us to observe memory access effects. Indeed, smaller might be better?
const PAYLOAD_SIZE_U64: usize = 1024 * 1024; // 1M u64 = 8 MB of useful payload

// TODO: Avoid hanging forever is barrier is not released.

struct BenchmarkRun {
    // Waited 3 times, once be each worker thread and once by runner thread.
    completed: Arc<Barrier>,

    join_handles: Box<[JoinHandle<()>]>,
}

enum RunConfiguration {
    SameMemoryRegion,
    DifferentMemoryRegion,
}

impl BenchmarkRun {
    fn new(processor_pair: &ProcessorSet, configuration: RunConfiguration) -> Self {
        assert_eq!(processor_pair.len(), 2);

        // To keep the runs very consistent with each other, we create two channels here and
        // depending on the run configuration wire them up cross-thread or same-thread, so each
        // thread executes the same logic, only with a different channel sender depending on where
        // its prepared data set needs to end up.

        // Numbered by receiver-thread mapping - thread 1 will use channel 1 receiver.
        let (c1_tx, c1_rx) = mpsc::channel::<Payload>();
        let (c2_tx, c2_rx) = mpsc::channel::<Payload>();

        let (thread1_sends_to, thread2_sends_to) = match configuration {
            RunConfiguration::SameMemoryRegion => (c1_tx, c2_tx),
            RunConfiguration::DifferentMemoryRegion => (c2_tx, c1_tx),
        };

        // Once by current thread, once by each worker.
        let ready = Arc::new(Barrier::new(3));

        // We stuff them in a bag. Whichever thread starts first takes the first group.
        let bag = Arc::new(Mutex::new(vec![
            (thread1_sends_to, c1_rx, Arc::clone(&ready)),
            (thread2_sends_to, c2_rx, Arc::clone(&ready)),
        ]));

        let completed = Arc::new(Barrier::new(3));

        // TODO: Should this be using an entrypoint factory? What if input is not cloneable?
        let join_handles = processor_pair.spawn_threads({
            let completed = Arc::clone(&completed);

            move |_| {
                let bag = Arc::clone(&bag);
                let completed = Arc::clone(&completed);

                let (tx, rx, ready) = bag.lock().unwrap().pop().unwrap();

                let payload = black_box(prepare_payload());
                tx.send(payload).unwrap();

                let payload = rx.recv().unwrap();

                black_box(clean_caches());

                ready.wait();

                _ = black_box(process_payload(&payload));

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

fn prepare_payload() -> Payload {
    let mut payload = HashMap::with_capacity(PAYLOAD_SIZE_U64);

    for i in 0..PAYLOAD_SIZE_U64 {
        payload.insert(i as u64, (i * 2) as u64);
    }

    payload
}

fn process_payload(payload: &Payload) -> u64 {
    let mut sum: u64 = 0;

    for k in 0..PAYLOAD_SIZE_U64 {
        sum = sum.wrapping_add(*payload.get(&(k as u64)).unwrap_or(&0));
    }

    sum
}

const CACHE_CLEANER_LEN_BYTES: usize = 2000 * 1024 * 1024;
const CACHE_CLEANER_LEN_U64: usize = CACHE_CLEANER_LEN_BYTES / mem::size_of::<u64>();
static CACHE_CLEANER: LazyLock<Vec<u64>> =
    LazyLock::new(|| vec![0x0102030401020304; CACHE_CLEANER_LEN_U64]);

/// As the whole point of this benchmark is to demonstrate that there is a difference when accessing
/// memory, we need to ensure that memory actually gets accessed - that the data is not simply
/// cached locally. This function will perform a large memory copy operation, which hopefully
/// trashes any cache that may be present.
fn clean_caches() -> u64 {
    // Big servers (which are our target) can have gigabytes of L3 cache! We need to crunch through
    // a lot of data to exercise it all and fill the entire CPU cache with garbage.

    // A memory copy should do the trick?
    let mut copied_cleaner = Vec::with_capacity(CACHE_CLEANER_LEN_U64);

    let source = CACHE_CLEANER.as_ptr();
    let destination = copied_cleaner.as_mut_ptr();

    // SAFETY: We reserved memory for the destination, are using the right length,
    // and they do not overlap. All is well.
    unsafe {
        std::ptr::copy_nonoverlapping(source, destination, CACHE_CLEANER_LEN_U64);
    }

    // SAFETY: Yeah, we just initialized it.
    unsafe {
        copied_cleaner.set_len(CACHE_CLEANER_LEN_U64);
    }

    // Paranoia to make sure the compiler doesn't optimize all our valuable work away.
    copied_cleaner[52251] + copied_cleaner[6353532]
}
