use std::{
    collections::VecDeque,
    hint::black_box,
    mem,
    num::NonZeroUsize,
    sync::{mpsc, Arc, Barrier, LazyLock, Mutex},
    thread::JoinHandle,
};

use criterion::{
    criterion_group, criterion_main, measurement::WallTime, BatchSize, BenchmarkGroup, Criterion,
};
use derive_more::derive::Display;
use folo_hw::ProcessorSet;
use itertools::Itertools;
use nonempty::nonempty;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

// TODO: Do not hang forever if worker panics. We need timeouts on barriers!

const ONE_PROCESSOR: NonZeroUsize = NonZeroUsize::new(1).unwrap();

fn entrypoint(c: &mut Criterion) {
    let mut g = c.benchmark_group("channel_exchange");

    execute_run::<ChannelExchange>(&mut g, WorkDistribution::MemoryRegionPairs);
    execute_run::<ChannelExchange>(&mut g, WorkDistribution::SameMemoryRegion);

    g.finish();
}

fn execute_run<P: Payload>(g: &mut BenchmarkGroup<'_, WallTime>, distribution: WorkDistribution) {
    // Probe whether we even have enough processors for this run. If not, just skip.
    if get_processor_pairs(distribution).is_none() {
        eprintln!("Skipping {distribution} - system hardware topology is not compatible.");
        return;
    }

    g.bench_function(distribution.to_string(), |b| {
        b.iter_batched(
            || {
                let processor_pairs = get_processor_pairs(distribution)
                    .expect("we already validated that we have the right topology");

                BenchmarkRun::new::<P>(&processor_pairs, distribution)
            },
            |run| run.wait(),
            BatchSize::PerIteration,
        );
    });
}

/// Identifies how many worker thread pairs we need to use in the benchmark,
/// based on the hardware topology of the system.
fn calculate_worker_pair_count() -> NonZeroUsize {
    // One pair for every memory region. That's it.
    NonZeroUsize::new(
        ProcessorSet::builder()
            .performance_processors_only()
            .take_all()
            .expect("must have at least one processor")
            .processors()
            .iter()
            .map(|p| p.memory_region_id())
            .unique()
            .count()
            * 2,
    )
    .expect("there must be at least one memory region")
}

/// Obtains the processor pairs to use for one iteration of the benchmark. We pick different
/// processors for different iterations to help average out any differences in performance
/// that may exist due to competing workloads.
fn get_processor_pairs(distribution: WorkDistribution) -> Option<Vec<ProcessorSet>> {
    let worker_pair_count = calculate_worker_pair_count();

    // If the system has efficiency processors, we do not want them.
    let candidates = ProcessorSet::builder()
        .performance_processors_only()
        .take_all()
        .expect("there must be at least one performance processor on any system by definition");

    match distribution {
        WorkDistribution::MemoryRegionPairs => {
            // We start by picking the first one of each pair.
            let first_processors = candidates
                .to_builder()
                .different_memory_regions()
                .take(worker_pair_count)?;

            // This must logically match our pair count because we have one pair per memory region
            // and expect to get one processor from each memory region with performance processors.
            assert_eq!(first_processors.len(), worker_pair_count.get());

            // Now we need to find a partner for each of the processors.
            let mut partners = first_processors
                .processors()
                .iter()
                .filter_map(|p| {
                    candidates
                        .to_builder()
                        .except([p])
                        .filter(|c| c.memory_region_id() == p.memory_region_id())
                        .take(ONE_PROCESSOR)
                })
                .collect::<VecDeque<_>>();

            if partners.len() != worker_pair_count.get() {
                // Some memory region did not have enough processors.
                return None;
            }

            // Each memory region needs to partner with its neighbors,
            // so we actually need to shift the partners around by one.
            let recycled_processor = partners
                .pop_front()
                .expect("we verified it has the right length");

            partners.push_back(recycled_processor);

            // Great success! Almost. We still need to package them up as a ProcessorSet.
            Some(
                first_processors
                    .processors()
                    .iter()
                    .zip(partners)
                    .map(|(&p1, p2_set)| {
                        let p2 = *p2_set.processors().first();
                        ProcessorSet::from_processors(nonempty![p1, p2])
                    })
                    .collect_vec(),
            )
        }
        WorkDistribution::SameMemoryRegion => {
            // Now the key question here is... which memory region do we pick?
            // We are going to assume it does not matter - the links between
            // memory regions can be bottlenecks but within one memory region
            // everything should still be norminal.
            Some(
                candidates
                    .to_builder()
                    .same_memory_region()
                    .take(
                        NonZeroUsize::new(worker_pair_count.get() * 2)
                            .expect("* 2 cannot make a number zero"),
                    )?
                    .processors()
                    .into_iter()
                    .chunks(2)
                    .into_iter()
                    .map(|pair| {
                        let pair = pair.collect_vec();
                        assert_eq!(pair.len(), 2);

                        ProcessorSet::from_processors(nonempty![pair[0], pair[1]])
                    })
                    .collect_vec(),
            )
        }
        WorkDistribution::SameProcessor => {
            // Here we do not care at all which processors are selected - work is
            // local on every processor, so just pick whatever.
            Some(
                candidates
                    .to_builder()
                    .take(
                        NonZeroUsize::new(worker_pair_count.get() * 2)
                            .expect("* 2 cannot make a number zero"),
                    )?
                    .processors()
                    .into_iter()
                    .chunks(2)
                    .into_iter()
                    .map(|pair| {
                        let pair = pair.collect_vec();
                        assert_eq!(pair.len(), 2);

                        ProcessorSet::from_processors(nonempty![pair[0], pair[1]])
                    })
                    .collect_vec(),
            )
        }
    }
}

#[derive(Debug)]
struct BenchmarkRun {
    // Waited 3 times, once be each worker thread and once by runner thread.
    completed: Arc<Barrier>,

    join_handles: Box<[JoinHandle<()>]>,
}

impl BenchmarkRun {
    fn new<P: Payload>(processor_pairs: &[ProcessorSet], distribution: WorkDistribution) -> Self {
        // Once by current thread + once by each worker in each pair.
        // All workers will start when all workers and the main thread are ready.
        let ready_signal = Arc::new(Barrier::new(1 + 2 * processor_pairs.len()));
        let completed_signal = Arc::new(Barrier::new(1 + 2 * processor_pairs.len()));

        let mut join_handles = Vec::with_capacity(2 * processor_pairs.len());

        for pair in processor_pairs {
            assert_eq!(pair.len(), 2);

            let (payload1, payload2) = P::new_pair();

            // We use these to deliver a prepared payload to the worker meant to process it.
            // Depending on the mode, we either wire up the channels to themselves or each other.
            let (c1_tx, c1_rx) = mpsc::channel::<P>();
            let (c2_tx, c2_rx) = mpsc::channel::<P>();

            let ((tx1, rx1), (tx2, rx2)) = match distribution {
                WorkDistribution::MemoryRegionPairs | WorkDistribution::SameMemoryRegion => {
                    // Pair 1 will send to pair 2 and vice versa.
                    ((c1_tx, c2_rx), (c2_tx, c1_rx))
                }
                WorkDistribution::SameProcessor => {
                    // Pair 1 will send to itself and pair 2 will send to itself.
                    ((c1_tx, c1_rx), (c2_tx, c2_rx))
                }
            };

            // We stuff everything in a bag. Whichever thread starts first takes the first group.
            let bag = Arc::new(Mutex::new(vec![(tx1, rx1, payload1), (tx2, rx2, payload2)]));

            join_handles.extend(pair.spawn_threads({
                let ready_signal = Arc::clone(&ready_signal);
                let completed_signal = Arc::clone(&completed_signal);

                move |_| {
                    let (payload_tx, payload_rx, mut payload) = bag.lock().unwrap().pop().unwrap();

                    payload.prepare();

                    // Potentially trade payloads with the other worker in the pair.
                    // This may or may not go anywhere - it might just send back to itself.
                    payload_tx.send(payload).unwrap();
                    let payload = payload_rx.recv().unwrap();

                    _ = black_box(clean_caches());

                    ready_signal.wait();

                    let _ = black_box(payload.process());

                    completed_signal.wait();
                }
            }));
        }

        ready_signal.wait();

        Self {
            completed: completed_signal,
            join_handles: join_handles.into_boxed_slice(),
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

trait Payload: Sized + Send + 'static {
    /// Creates the payload pair that will be used to initialize one worker pair in one
    /// benchmark iteration. This will be called on the main thread.
    fn new_pair() -> (Self, Self);

    /// Performs any initialization required. This will be called before the benchmark time span
    /// measurement starts. It will be called on a worker thread but the payload may be moved to
    /// a different worker thread before the benchmark starts (as workers by default prepare work
    /// for each other, to showcase what happens when the work is transferred further along).
    fn prepare(&mut self);

    /// Processes the payload, consuming it. The iteration is complete when this returns.
    ///
    /// The return value is meaningless, just there to deter optimizations
    /// that the compiler might be inclined to make if it saw no data coming out.
    fn process(self) -> u64;
}

#[derive(Copy, Clone, Debug, Display, Eq, PartialEq)]
enum WorkDistribution {
    /// One worker pair is spawned for each numerically neighboring memory region pair.
    /// Each pair will work together, organizing work between the two members. In total,
    /// there will be two workers per memory region (one working with the "previous"
    /// memory region and one working with the "next" one).
    ///
    /// Different memory regions may be a different distance apart, so this allows us to average
    /// out any differences - some pairs are faster, some are slower, we just want to average it
    /// out so every benchmark run is consistent (instead of picking two random memory regions).
    ///
    /// This option can only be used if there are at least two memory regions.
    MemoryRegionPairs,

    /// All pairs of workers are spawned in the same memory region. Each pair will work
    /// together, organizing work between the two members.
    ///
    /// The number of pairs will match the number that would have been used with
    /// `MemoryRegionPairs`, for optimal comparability. There will be a minimum of one pair.
    SameMemoryRegion,

    /// Like SameMemoryRegion but each worker is given back its own payload. We still have the
    /// same total number of workers to keep total system load equivalent.
    SameProcessor,
}

// Large servers can make hundreds of MBs of L3 cache available to a single core, though it
// depends on the specific model and hardware configuration. We try a sufficiently large cache
// thrashing here to have a good chance of evicting the data we are interested in from the cache.
const CACHE_CLEANER_LEN_BYTES: usize = 128 * 1024 * 1024;
const CACHE_CLEANER_LEN_U64: usize = CACHE_CLEANER_LEN_BYTES / mem::size_of::<u64>();
static CACHE_CLEANER: LazyLock<Vec<u64>> =
    LazyLock::new(|| vec![0x0102030401020304; CACHE_CLEANER_LEN_U64]);

/// As the whole point of this benchmark is to demonstrate that there is a difference when accessing
/// memory, we need to ensure that memory actually gets accessed - that the data is not simply
/// cached locally. This function will perform a large memory copy operation, which hopefully
/// trashes any cache that may be present.
fn clean_caches() -> u64 {
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

/// The two paired workers exchange messages with each other, sending back whatever is received.
/// Note: SameProcessor mode is not supported because the workers must collaborate in real time.
#[derive(Debug)]
struct ChannelExchange {
    // Anything received here...
    rx: mpsc::Receiver<u64>,

    // ...is sent back here.
    tx: mpsc::Sender<u64>,
}

impl Payload for ChannelExchange {
    fn new_pair() -> (Self, Self) {
        let (c1_tx, c1_rx) = mpsc::channel::<u64>();
        let (c2_tx, c2_rx) = mpsc::channel::<u64>();

        let worker1 = Self {
            rx: c1_rx,
            tx: c2_tx,
        };
        let worker2 = Self {
            rx: c2_rx,
            tx: c1_tx,
        };

        (worker1, worker2)
    }

    fn prepare(&mut self) {
        const INITIAL_BUFFERED_MESSAGE_COUNT: usize = 10_000;

        for i in 0..INITIAL_BUFFERED_MESSAGE_COUNT {
            self.tx.send(i as u64).unwrap();
        }
    }

    fn process(self) -> u64 {
        const MESSAGE_PUMP_ITERATION_COUNT: usize = 500_000;

        for _ in 0..MESSAGE_PUMP_ITERATION_COUNT {
            let payload = self.rx.recv().unwrap();

            // It is OK for this to return Err when shutting down (the partner worker has already
            // exited and cleaned up, so the message will never be received - which is fine).
            _ = self.tx.send(payload);
        }

        MESSAGE_PUMP_ITERATION_COUNT as u64
    }
}
