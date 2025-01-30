use std::{
    collections::{HashMap, VecDeque},
    env,
    hint::black_box,
    mem::{self, MaybeUninit},
    num::NonZeroUsize,
    sync::{mpsc, Arc, Barrier, LazyLock, Mutex},
    thread::JoinHandle,
    time::Duration,
};

use criterion::{
    criterion_group, criterion_main, measurement::WallTime, BatchSize, BenchmarkGroup, Criterion,
};
use derive_more::derive::Display;
use fake_headers::Headers;
use folo_hw::{cpulist, ProcessorSet};
use frozen_collections::{FzHashMap, FzScalarMap, MapQuery};
use http::{HeaderMap, HeaderName, HeaderValue};
use itertools::Itertools;
use nonempty::nonempty;

// https://github.com/cloudhead/nonempty/issues/68
extern crate alloc;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

// TODO: Do not hang forever if worker panics. We need timeouts on barriers!

const ONE_PROCESSOR: NonZeroUsize = NonZeroUsize::new(1).unwrap();
const TWO_PROCESSORS: NonZeroUsize = NonZeroUsize::new(2).unwrap();

fn entrypoint(c: &mut Criterion) {
    let mut g = c.benchmark_group("channel_exchange");

    execute_run::<ChannelExchange, 1>(&mut g, WorkDistribution::PinnedMemoryRegionPairs);
    execute_run::<ChannelExchange, 1>(&mut g, WorkDistribution::PinnedSameMemoryRegion);
    execute_run::<ChannelExchange, 1>(&mut g, WorkDistribution::UnpinnedMemoryRegionPairs);
    execute_run::<ChannelExchange, 1>(&mut g, WorkDistribution::UnpinnedSameMemoryRegion);

    g.finish();
    let mut g = c.benchmark_group("hashmap_read");

    execute_run::<HashMapRead, 10>(&mut g, WorkDistribution::PinnedMemoryRegionPairs);
    execute_run::<HashMapRead, 10>(&mut g, WorkDistribution::PinnedSameMemoryRegion);
    execute_run::<HashMapRead, 10>(&mut g, WorkDistribution::PinnedSelf);
    execute_run::<HashMapRead, 10>(&mut g, WorkDistribution::UnpinnedMemoryRegionPairs);
    execute_run::<HashMapRead, 10>(&mut g, WorkDistribution::UnpinnedSameMemoryRegion);
    execute_run::<HashMapRead, 10>(&mut g, WorkDistribution::UnpinnedSelf);

    g.finish();
    let mut g = c.benchmark_group("fz_hashmap_read");

    execute_run::<FzHashMapRead, 50>(&mut g, WorkDistribution::PinnedMemoryRegionPairs);
    execute_run::<FzHashMapRead, 50>(&mut g, WorkDistribution::PinnedSameMemoryRegion);
    execute_run::<FzHashMapRead, 50>(&mut g, WorkDistribution::PinnedSelf);
    execute_run::<FzHashMapRead, 50>(&mut g, WorkDistribution::UnpinnedMemoryRegionPairs);
    execute_run::<FzHashMapRead, 50>(&mut g, WorkDistribution::UnpinnedSameMemoryRegion);
    execute_run::<FzHashMapRead, 50>(&mut g, WorkDistribution::UnpinnedSelf);

    g.finish();
    let mut g = c.benchmark_group("fz_scalarmap_read");

    execute_run::<FzScalarMapRead, 50>(&mut g, WorkDistribution::PinnedMemoryRegionPairs);
    execute_run::<FzScalarMapRead, 50>(&mut g, WorkDistribution::PinnedSameMemoryRegion);
    execute_run::<FzScalarMapRead, 50>(&mut g, WorkDistribution::PinnedSelf);
    execute_run::<FzScalarMapRead, 50>(&mut g, WorkDistribution::UnpinnedMemoryRegionPairs);
    execute_run::<FzScalarMapRead, 50>(&mut g, WorkDistribution::UnpinnedSameMemoryRegion);
    execute_run::<FzScalarMapRead, 50>(&mut g, WorkDistribution::UnpinnedSelf);

    g.finish();
    let mut g = c.benchmark_group("http_headers_parse");

    execute_run::<HttpHeadersParse, 1>(&mut g, WorkDistribution::PinnedMemoryRegionPairs);
    execute_run::<HttpHeadersParse, 1>(&mut g, WorkDistribution::PinnedSameMemoryRegion);
    execute_run::<HttpHeadersParse, 1>(&mut g, WorkDistribution::PinnedSelf);
    execute_run::<HttpHeadersParse, 1>(&mut g, WorkDistribution::UnpinnedMemoryRegionPairs);
    execute_run::<HttpHeadersParse, 1>(&mut g, WorkDistribution::UnpinnedSameMemoryRegion);
    execute_run::<HttpHeadersParse, 1>(&mut g, WorkDistribution::UnpinnedSelf);

    g.finish();
}

fn is_fake_run() -> bool {
    env::args().any(|a| a == "--test" || a == "--list")
}

fn execute_run<P: Payload, const PAYLOAD_MULTIPLIER: usize>(
    g: &mut BenchmarkGroup<'_, WallTime>,
    distribution: WorkDistribution,
) {
    // Writing to stderr during listing/testing leads to test runner errors because it expects
    // a special protocol to be spoken, so we only emit this output during actual execution.
    if !is_fake_run() {
        // Probe whether we even have enough processors for this run. If not, just skip.
        let Some(sample_processor_selection) = get_processor_set_pairs(distribution) else {
            println!("Skipping {distribution} - system hardware topology is not compatible.");
            return;
        };

        // Print a reference of what sort of processors are selected for this scenario.
        // Just to help a human reader get a feel for what is configured.
        // This selection is discarded - each iteration will make a new selection.
        for (processor_set_1, processor_set_2) in sample_processor_selection {
            let cpulist1 = cpulist::emit(processor_set_1.processors().map(|p| p.id()));
            let cpulist2 = cpulist::emit(processor_set_2.processors().map(|p| p.id()));

            println!("{distribution} reference selection: ({cpulist1}) & ({cpulist2})");
        }
    }

    // These benchmarks can be pretty slow and clearing processor caches adds extra overhead
    // between iterations, so to get stable and consistent data it is worth taking some time.
    g.measurement_time(Duration::from_secs(60));

    g.bench_function(format!("{distribution} (x{PAYLOAD_MULTIPLIER})"), |b| {
        b.iter_batched(
            || {
                let processor_set_pairs = get_processor_set_pairs(distribution)
                    .expect("we already validated that we have the right topology");

                BenchmarkRun::new::<P, PAYLOAD_MULTIPLIER>(&processor_set_pairs, distribution)
            },
            // We explicitly return the run back to the benchmark infrastructure to ensure that any
            // time spent in dropping the payload (between completed_signal and worker thread joins)
            // is not counted into the benchmark time itself.
            |run| {
                run.wait();
                run
            },
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
            .count(),
    )
    .expect("there must be at least one memory region")
}

/// Obtains the processor pairs to use for one iteration of the benchmark. We pick different
/// processors for different iterations to help average out any differences in performance
/// that may exist due to competing workloads.
///
/// For the "pinned" variants, this returns for each pair of workers a pair of single-processor
/// ProcessorSets. For the "unpinned" variants, this returns for each pair of workers a pair of
/// many-processor ProcessorSets.
fn get_processor_set_pairs(
    distribution: WorkDistribution,
) -> Option<Vec<(ProcessorSet, ProcessorSet)>> {
    let worker_pair_count = calculate_worker_pair_count();

    // If the system has efficiency processors, we do not want them.
    let candidates = ProcessorSet::builder()
        .performance_processors_only()
        .take_all()
        .expect("there must be at least one performance processor on any system by definition");

    match distribution {
        WorkDistribution::PinnedMemoryRegionPairs => {
            // If there is only one pair, this means there is only one memory region, in which
            // case this distribution mode is meaningless and we will not execute.
            if worker_pair_count.get() == 1 {
                return None;
            }

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

            // Great success! Almost. We still need to package them up as pairs of ProcessorSets.
            Some(
                first_processors
                    .processors()
                    .iter()
                    .zip(partners)
                    .map(|(&p1, p2_set)| {
                        let set1 = ProcessorSet::from_processors(nonempty![p1]);
                        (set1, p2_set)
                    })
                    .collect_vec(),
            )
        }
        WorkDistribution::PinnedSameMemoryRegion => {
            // We remove processors that have already been picked, so each pair
            // gets two unique processors from the same memory region.
            let mut remaining_candidates = candidates.clone();

            let mut result = Vec::with_capacity(worker_pair_count.get());

            for _ in 0..worker_pair_count.get() {
                let pair = remaining_candidates
                    .to_builder()
                    .same_memory_region()
                    .take(TWO_PROCESSORS)?;

                remaining_candidates = match remaining_candidates
                    .to_builder()
                    .except(&pair.processors())
                    .take_all()
                {
                    Some(remaining) => remaining,
                    None => {
                        // We ran out of processors before we could finish.
                        return None;
                    }
                };

                let pair = pair.processors();
                assert_eq!(pair.len(), 2);

                let set1: ProcessorSet = pair[0].into();
                let set2: ProcessorSet = pair[1].into();

                result.push((set1, set2));
            }

            Some(result)
        }
        WorkDistribution::PinnedSelf => {
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

                        let set1: ProcessorSet = pair[0].into();
                        let set2: ProcessorSet = pair[1].into();

                        (set1, set2)
                    })
                    .collect_vec(),
            )
        }
        WorkDistribution::UnpinnedMemoryRegionPairs => {
            // If there is only one pair, this means there is only one memory region, in which
            // case this distribution mode is meaningless and we will not execute.
            if worker_pair_count.get() == 1 {
                return None;
            }

            // We start by picking the first one of each pair.
            let first_processors = candidates
                .to_builder()
                .different_memory_regions()
                .take(worker_pair_count)?;

            // This must logically match our pair count because we have one pair per memory region
            // and expect to get one processor from each memory region with performance processors.
            assert_eq!(first_processors.len(), worker_pair_count.get());

            // Now we expand each single processor into "all the processors in that memory region".
            let first_processor_sets = first_processors
                .processors()
                .into_iter()
                .map(|p| {
                    candidates
                        .to_builder()
                        .filter(|c| c.memory_region_id() == p.memory_region_id())
                        .take_all()
                        .expect("must have at least one processor in every active memory region")
                })
                .collect::<VecDeque<_>>();

            assert_eq!(first_processor_sets.len(), worker_pair_count.get());

            // Clone the whole thing to start making the partner sets.
            let mut partners = first_processor_sets.clone();

            // Each memory region needs to partner with its neighbors,
            // so we actually need to shift the partners around by one.
            let recycled_processor_set = partners
                .pop_front()
                .expect("we verified it has the right length");

            partners.push_back(recycled_processor_set);

            // Great success! Almost. We still need to package them up as pairs of ProcessorSets.
            Some(
                first_processors
                    .processors()
                    .iter()
                    .zip(partners)
                    .map(|(&p1, p2_set)| {
                        let set1 = ProcessorSet::from_processors(nonempty![p1]);
                        (set1, p2_set)
                    })
                    .collect_vec(),
            )
        }
        WorkDistribution::UnpinnedSameMemoryRegion => {
            // We remove processors that have already been picked, so each pair
            // gets two processors' worth of processor time from the same memory region.
            // We do not pin them but we do ensure there are enough processors to "fit"
            // all the worker pairs into the memory region, without oversubscribing.
            let mut remaining_candidates = candidates.clone();

            let mut result = Vec::with_capacity(worker_pair_count.get());

            for _ in 0..worker_pair_count.get() {
                let pair = remaining_candidates
                    .to_builder()
                    .same_memory_region()
                    .take(TWO_PROCESSORS)?;

                remaining_candidates = match remaining_candidates
                    .to_builder()
                    .except(&pair.processors())
                    .take_all()
                {
                    Some(remaining) => remaining,
                    None => {
                        // We ran out of processors before we could finish.
                        return None;
                    }
                };

                let pair = pair.processors();
                assert_eq!(pair.len(), 2);

                // For each pair we re-select all the processors in the same memory region
                // and use this entire set for both workers. The original pair was just to
                // point to the memory region and is now discarded.

                let real_set = candidates
                    .to_builder()
                    .filter(|c| c.memory_region_id() == pair[0].memory_region_id())
                    .take_all()
                    .expect("must have at least one processor in every active memory region");

                result.push((real_set.clone(), real_set));
            }

            Some(result)
        }
        WorkDistribution::UnpinnedSelf => {
            // All processors are valid candidates here.
            Some(
                (0..worker_pair_count.get())
                    .map(|_| (candidates.clone(), candidates.clone()))
                    .collect(),
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
    /// Some benchmark scenarios can be a bit fast but cannot be "naturally" sized up because they
    /// need to meet some criteria like fitting in CPU caches. `PAYLOAD_MULTIPLIER > 1` will simply
    /// execute the benchmark multiple times in sequence to collect more data, with a different
    /// payload each time but on the same workers. The expectation is that each payload is
    /// independent and does not require cache cleaning between payloads for meaningful results.
    fn new<P: Payload, const PAYLOAD_MULTIPLIER: usize>(
        processor_set_pairs: &[(ProcessorSet, ProcessorSet)],
        distribution: WorkDistribution,
    ) -> Self {
        // Once by current thread + once by each worker in each pair.
        // All workers will start when all workers and the main thread are ready.
        let ready_signal = Arc::new(Barrier::new(1 + 2 * processor_set_pairs.len()));
        let completed_signal = Arc::new(Barrier::new(1 + 2 * processor_set_pairs.len()));

        let mut join_handles = Vec::with_capacity(2 * processor_set_pairs.len());

        for processor_set_pair in processor_set_pairs {
            let (processor_set_1, processor_set_2) = processor_set_pair;

            let (payloads1, payloads2) = (0..PAYLOAD_MULTIPLIER).map(|_| P::new_pair()).unzip();

            // We use these to deliver a prepared payload to the worker meant to process it.
            // Depending on the mode, we either wire up the channels to themselves or each other.
            let (c1_tx, c1_rx) = mpsc::channel::<Vec<P>>();
            let (c2_tx, c2_rx) = mpsc::channel::<Vec<P>>();

            let ((tx1, rx1), (tx2, rx2)) = match distribution {
                WorkDistribution::PinnedMemoryRegionPairs
                | WorkDistribution::PinnedSameMemoryRegion
                | WorkDistribution::UnpinnedMemoryRegionPairs
                | WorkDistribution::UnpinnedSameMemoryRegion => {
                    // Pair 1 will send to pair 2 and vice versa.
                    ((c1_tx, c2_rx), (c2_tx, c1_rx))
                }
                WorkDistribution::PinnedSelf | WorkDistribution::UnpinnedSelf => {
                    // Pair 1 will send to itself and pair 2 will send to itself.
                    ((c1_tx, c1_rx), (c2_tx, c2_rx))
                }
            };

            // We stuff everything in a bag. Whichever thread starts first takes the first group.
            let bag = Arc::new(Mutex::new(vec![
                (tx1, rx1, payloads1),
                (tx2, rx2, payloads2),
            ]));

            join_handles.push(BenchmarkRun::spawn_worker(
                processor_set_1,
                Arc::clone(&ready_signal),
                Arc::clone(&completed_signal),
                Arc::clone(&bag),
            ));
            join_handles.push(BenchmarkRun::spawn_worker(
                processor_set_2,
                Arc::clone(&ready_signal),
                Arc::clone(&completed_signal),
                Arc::clone(&bag),
            ));
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

    #[expect(clippy::type_complexity)] // True but whatever, do not care about it here.
    fn spawn_worker<P: Payload>(
        processor_set: &ProcessorSet,
        ready_signal: Arc<Barrier>,
        completed_signal: Arc<Barrier>,
        payload_bag: Arc<Mutex<Vec<(mpsc::Sender<Vec<P>>, mpsc::Receiver<Vec<P>>, Vec<P>)>>>,
    ) -> JoinHandle<()> {
        processor_set.spawn_thread({
            move |_| {
                let (payloads_tx, payloads_rx, mut payloads) =
                    payload_bag.lock().unwrap().pop().unwrap();

                for payload in &mut payloads {
                    payload.prepare();
                }

                // Potentially trade payloads with the other worker in the pair.
                // This may or may not go anywhere - it might just send back to itself.
                payloads_tx.send(payloads).unwrap();
                let mut payloads = payloads_rx.recv().unwrap();

                _ = black_box(clean_caches());

                ready_signal.wait();

                for payload in &mut payloads {
                    payload.process();
                }

                completed_signal.wait();

                // The payloads are dropped at the end, after the benchmark time span
                // measurement ends due to all threads reaching completed_signal.
                drop(payloads);
            }
        })
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

    /// Processes the payload but does not consume it. The iteration is complete when this returns
    /// for all payloads. The payloads are dropped later, to ensure that the benchmark time is not
    /// affected by the time it takes to drop the payload and release the memory.
    fn process(&mut self);
}

#[derive(Copy, Clone, Debug, Display, Eq, PartialEq)]
enum WorkDistribution {
    /// One worker pair is spawned for each numerically neighboring memory region pair.
    /// Each pair will work together, organizing work between the two members. In total,
    /// there will be two workers per memory region (one working with the "previous"
    /// memory region and one working with the "next" one).
    ///
    /// Each worker is pinned to a specific processor.
    ///
    /// Different memory regions may be a different distance apart, so this allows us to average
    /// out any differences - some pairs are faster, some are slower, we just want to average it
    /// out so every benchmark run is consistent (instead of picking two random memory regions).
    ///
    /// This option can only be used if there are at least two memory regions.
    PinnedMemoryRegionPairs,

    /// Each worker in a pair is spawned in the same memory region. Each pair will work
    /// together, organizing work between the two members. Different pairs may be in different
    /// memory regions.
    ///
    /// Each worker is pinned to a specific processor.
    ///
    /// The number of pairs will match the number that would have been used with
    /// `PinnedMemoryRegionPairs`, for optimal comparability. There will be a minimum of one pair.
    PinnedSameMemoryRegion,

    /// All workers are spawned without regard to memory region or processor, randomly picking
    /// processors for each iteration.
    ///
    /// Each worker is given back its own payload. We still have the
    /// same total number of workers to keep total system load equivalent.
    PinnedSelf,

    /// Like `PinnedMemoryRegionPairs` but each worker is allowed to float among all the processors
    /// in the memory region, based on the operating system's scheduling decisions.
    ///
    /// We still have the same total number of workers to keep total system load equivalent.
    UnpinnedMemoryRegionPairs,

    /// Like `PinnedSameMemoryRegion` but each worker is allowed to float among all the processors
    /// in the memory region, based on the operating system's scheduling decisions.
    ///
    /// We still have the same total number of workers to keep total system load equivalent.
    UnpinnedSameMemoryRegion,

    /// All workers are spawned without regard to memory region or processor, leaving it up
    /// to the operating system to decide where to run them. Note that, typically, this will still
    /// result in them running in the same memory region, as that tends to be the default behavior.
    ///
    /// Each worker is given back its own payload. We still have the
    /// same total number of workers to keep total system load equivalent.
    UnpinnedSelf,
}

// Large servers can make hundreds of MBs of L3 cache available to a single core, though it
// depends on the specific model and hardware configuration. We try a sufficiently large cache
// thrashing here to have a good chance of evicting the data we are interested in from the cache.
const CACHE_CLEANER_LEN_BYTES: usize = 128 * 1024 * 1024;
const CACHE_CLEANER_LEN_U64: usize = CACHE_CLEANER_LEN_BYTES / mem::size_of::<u64>();
// We copy the memory in small pieces to avoid having to do a big temporary allocation, which
// would just slow down the benchmark. The processor does not really care where it is moving the
// bytes to, we do not need to keep these bytes around for long.
const CACHE_CLEANER_CHUNK_LEN_BYTES: usize = 128 * 1024;
const CACHE_CLEANER_CHUNK_LEN_U64: usize = CACHE_CLEANER_CHUNK_LEN_BYTES / mem::size_of::<u64>();
static CACHE_CLEANER: LazyLock<Vec<u64>> =
    LazyLock::new(|| vec![0x0102030401020304; CACHE_CLEANER_LEN_U64]);

/// As the whole point of this benchmark is to demonstrate that there is a difference when accessing
/// memory, we need to ensure that memory actually gets accessed - that the data is not simply
/// cached locally. This function will perform a large memory copy operation, which hopefully
/// trashes any cache that may be present.
pub fn clean_caches() -> u64 {
    // A memory copy should do the trick? Stack-allocate the memory to avoid heap overhead here.
    let mut buffer = [MaybeUninit::<u64>::uninit(); CACHE_CLEANER_CHUNK_LEN_U64];

    let mut u64_remaining = CACHE_CLEANER_LEN_U64;

    let mut source = CACHE_CLEANER.as_ptr();
    let destination = buffer.as_mut_ptr().cast();

    while u64_remaining > 0 {
        let chunk_len_u64 = u64_remaining.min(CACHE_CLEANER_CHUNK_LEN_U64);

        // SAFETY: We reserved memory for the destination, are using the right length,
        // and they do not overlap. All is well.
        unsafe {
            std::ptr::copy_nonoverlapping(source, destination, chunk_len_u64);
        }

        u64_remaining -= chunk_len_u64;
        // SAFETY: The math checks out. We are operating in u64 everywhere.
        source = unsafe { source.add(chunk_len_u64) };
    }

    // Paranoia to make sure the compiler doesn't optimize all our valuable work away.
    source as u64
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

    fn process(&mut self) {
        const MESSAGE_PUMP_ITERATION_COUNT: usize = 500_000;

        for _ in 0..MESSAGE_PUMP_ITERATION_COUNT {
            let payload = self.rx.recv().unwrap();

            // It is OK for this to return Err when shutting down (the partner worker has already
            // exited and cleaned up, so the message will never be received - which is fine).
            _ = self.tx.send(payload);
        }
    }
}

// The logic behind this sizing is that we want to show "meaningful" numbers that actually show
// off the memory-access-friendliness of the different map data structures instead of just making
// it a benchmark of "how fast is reading 100s of MB from main memory". This means the data set
// should be small enough to be cacheable - we care about how fast it gets into the local caches
// and how well the data structure structures its memory accesses, to give the different data
// structures the opportunity to compete instead of just making it about how fast the
// "picking of entries from cold memory" part itself takes (which is trivial to measure but also
// better to measure with hardware manufacturer provided tools - pure memory throughput is easy).
const MAP_ENTRY_COUNT: usize = 128 * 1024; // 128K x u64 = 1 MB of useful payload, cache-friendly

/// The first worker generates a hashmap; the other reads all elements from it.
#[derive(Debug, Default)]
struct HashMapRead {
    map: HashMap<u64, u64>,
}

impl Payload for HashMapRead {
    fn new_pair() -> (Self, Self) {
        (Self::default(), Self::default())
    }

    fn prepare(&mut self) {
        for i in 0..MAP_ENTRY_COUNT {
            self.map.insert(i as u64, (i * 2) as u64);
        }
    }

    fn process(&mut self) {
        for k in 0..MAP_ENTRY_COUNT {
            black_box(self.map.get(&(k as u64)));
        }
    }
}

/// The first worker generates a frozen hashmap; the other reads all elements from it.
#[derive(Debug, Default)]
struct FzHashMapRead {
    map: FzHashMap<u64, u64>,
}

impl Payload for FzHashMapRead {
    fn new_pair() -> (Self, Self) {
        (Self::default(), Self::default())
    }

    fn prepare(&mut self) {
        let entries = (0..MAP_ENTRY_COUNT)
            .map(|i| (i as u64, (i * 2) as u64))
            .collect_vec();

        self.map = FzHashMap::new(entries);
    }

    fn process(&mut self) {
        for k in 0..MAP_ENTRY_COUNT {
            black_box(self.map.get(&(k as u64)));
        }
    }
}

/// The first worker generates a frozen scalar map; the other reads all elements from it.
#[derive(Debug, Default)]
struct FzScalarMapRead {
    map: FzScalarMap<u64, u64>,
}

impl Payload for FzScalarMapRead {
    fn new_pair() -> (Self, Self) {
        (Self::default(), Self::default())
    }

    fn prepare(&mut self) {
        let entries = (0..MAP_ENTRY_COUNT)
            .map(|i| (i as u64, (i * 2) as u64))
            .collect_vec();

        self.map = FzScalarMap::new(entries);
    }

    fn process(&mut self) {
        for k in 0..MAP_ENTRY_COUNT {
            black_box(self.map.get(&(k as u64)));
        }
    }
}

const HEADERS_COUNT: usize = 10_000;

#[derive(Debug, Default)]
struct HttpHeadersParse {
    serialized: Option<Vec<Vec<u8>>>,
}

impl Payload for HttpHeadersParse {
    fn new_pair() -> (Self, Self) {
        (Self::default(), Self::default())
    }

    fn prepare(&mut self) {
        self.serialized = Some(
            (0..HEADERS_COUNT)
                .map(|_| {
                    let headers = Headers::default().generate();

                    let mut result = String::new();

                    for (key, value) in headers {
                        result.push_str(&format!("{key}: {value}\r\n"));
                    }

                    result.into_bytes()
                })
                .collect_vec(),
        );
    }

    /// Memory allocation appears to dominate this scenario - it needs to allocate a bunch of
    /// fresh memory for the headers and header strings, which just makes this a memory allocation
    /// benchmark. The actual parsing is trivial in comparison.
    ///
    /// Value: low.
    fn process(&mut self) {
        for serialized in self.serialized.take().unwrap() {
            // SAFETY: We serialized proper utf-8, it is fine.
            let serialized_str = unsafe { String::from_utf8_unchecked(serialized) };

            let mut headers = HeaderMap::new();
            for line in serialized_str.lines() {
                if let Some((key, value)) = line.split_once(':') {
                    let key = key.trim();
                    let value = value.trim();
                    let header_name = HeaderName::from_bytes(key.as_bytes()).unwrap();
                    let header_value = HeaderValue::from_str(value).unwrap();
                    headers.insert(header_name, header_value);
                }
            }

            black_box(headers);
        }
    }
}
