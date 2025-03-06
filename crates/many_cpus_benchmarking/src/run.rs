use std::{
    any::type_name,
    collections::VecDeque,
    env, mem,
    num::NonZero,
    sync::{Arc, Barrier, Mutex, mpsc},
    thread::JoinHandle,
    time::Duration,
};

use criterion::{BatchSize, BenchmarkGroup, Criterion, SamplingMode, measurement::WallTime};
use itertools::Itertools;
use many_cpus::ProcessorSet;
use nonempty::{NonEmpty, nonempty};
use rand::{rng, seq::SliceRandom};

use crate::{Payload, WorkDistribution, clean_caches};

// https://github.com/cloudhead/nonempty/issues/68
extern crate alloc;

/// Executes a number of benchmark runs for a specific payload type, using the specified work
/// distribution modes.
///
/// `PAYLOAD_MULTIPLIER` can be used to increase the size of the payload data set, to easily compare
/// how multiplying the data set size affects the duration of the scenario. This will generate
/// multiple payloads, each processed sequentially by every worker. Caches are not cleaned between
/// payloads since each payload is unique and will not be cached.
pub fn execute_runs<P: Payload, const PAYLOAD_MULTIPLIER: usize>(
    c: &mut Criterion,
    work_distributions: &[WorkDistribution],
) {
    let mut g = c.benchmark_group(type_name::<P>());

    // Many-processor benchmarks can be slow and clearing processor caches adds extra overhead
    // between iterations, so to get stable and consistent data it is worth taking some time.
    g.measurement_time(Duration::from_secs(60));

    // Unclear what exactly this does but the docs say for long-running benchmarks Flat is good.
    // Many-processor benchmarks can be extremely long-running, so okay, let's believe it.
    g.sampling_mode(SamplingMode::Flat);

    for &distribution in work_distributions {
        execute_run::<P, PAYLOAD_MULTIPLIER>(&mut g, distribution);
    }

    g.finish();
}

/// In some execution modes, we are only executing to list the benchmarks or to perform a dummy
/// run of a single iteration to test that it works. In these cases, we do not want to emit any
/// additional output to stderr because it will confuse the test runner.
fn is_fake_run() -> bool {
    env::args().any(|a| a == "--test" || a == "--list")
}

fn execute_run<P: Payload, const PAYLOAD_MULTIPLIER: usize>(
    g: &mut BenchmarkGroup<'_, WallTime>,
    work_distribution: WorkDistribution,
) {
    // Probe whether we even have enough processors for this run. If not, just skip.
    // This is just a sample - we throw this selection away after we verify we can generate it.
    let Some(sample_processor_selection) = get_processor_set_pairs(work_distribution) else {
        if !is_fake_run() {
            // Be silent if it is a fake run, to avoid confusing the test runner.
            eprintln!("Skipping {work_distribution} - system hardware topology is not compatible.");
        }

        return;
    };

    // Writing to stderr during listing/testing leads to test runner errors because it expects
    // a special protocol to be spoken, so we only emit this debug output during actual execution.
    if !is_fake_run() {
        // Print a reference of what sort of processors are selected for this scenario.
        // Just to help a human reader get a feel for what is configured.
        // This selection is discarded - each iteration will make a new selection.
        for (processor_set_1, processor_set_2) in sample_processor_selection {
            let cpulist1 = cpulist::emit(
                &processor_set_1
                    .processors()
                    .iter()
                    .map(|p| p.id())
                    .collect_vec(),
            );
            let cpulist2 = cpulist::emit(
                &processor_set_2
                    .processors()
                    .iter()
                    .map(|p| p.id())
                    .collect_vec(),
            );

            eprintln!("{work_distribution} reference selection: ({cpulist1}) & ({cpulist2})");
        }
    }

    let name = if PAYLOAD_MULTIPLIER == 1 {
        format!("{work_distribution}")
    } else {
        format!("{work_distribution}(x{PAYLOAD_MULTIPLIER})")
    };

    g.bench_function(name, |b| {
        b.iter_batched(
            || {
                let processor_set_pairs = get_processor_set_pairs(work_distribution)
                    .expect("we already validated that we have the right topology");

                BenchmarkRun::new::<P, PAYLOAD_MULTIPLIER>(&processor_set_pairs, work_distribution)
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

/// Identifies how many worker thread pairs we need to use in the benchmark, based on the hardware
/// topology of the system, using the "pinned memory region pairs" reference scenario.
///
/// All work distributions use the same number of pairs as the reference scenario, for
/// optimal comparability between different distributions.
fn calculate_worker_pair_count() -> NonZero<usize> {
    // One pair for every memory region. That's it.
    NonZero::new(
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

const ONE_PROCESSOR: NonZero<usize> = NonZero::new(1).unwrap();

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
        .expect("there must be at least one performance processor on any system, by definition");

    match distribution {
        WorkDistribution::PinnedMemoryRegionPairs => {
            // If there is only one pair requested, this means there is only one memory region,
            // in which case this distribution mode is meaningless and we will not execute.
            if worker_pair_count.get() == 1 {
                return None;
            }

            // We start by picking the first item in each pair.
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
                    .cloned()
                    .zip(partners)
                    .map(|(p1, p2_set)| {
                        let set1 = ProcessorSet::from_processors(nonempty![p1]);
                        (set1, p2_set)
                    })
                    .collect_vec(),
            )
        }
        WorkDistribution::PinnedSameMemoryRegion => {
            // We start by picking the first item in each pair. We still distribute the pairs
            // across all memory regions to even out the load and any hardware differences, even
            // though we do not actually care about crossing memory regions during operation.
            let first_processors = candidates
                .to_builder()
                .different_memory_regions()
                .take(worker_pair_count)?;

            // This must logically match our pair count because we have one pair per memory region
            // and expect to get one processor from each memory region with performance processors.
            assert_eq!(first_processors.len(), worker_pair_count.get());

            // Now we need to find a partner for each of the processors.
            let partners = first_processors
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

            // Great success! Almost. We still need to package them up as pairs of ProcessorSets.
            Some(
                first_processors
                    .processors()
                    .iter()
                    .cloned()
                    .zip(partners)
                    .map(|(p1, p2_set)| {
                        let set1 = ProcessorSet::from_processors(nonempty![p1]);
                        (set1, p2_set)
                    })
                    .collect_vec(),
            )
        }
        WorkDistribution::PinnedSelf => {
            // Here we do not care at all which processors are selected - work is
            // local on every processor, so just pick arbitrary pairs.
            Some(
                candidates
                    .to_builder()
                    .take(
                        NonZero::new(worker_pair_count.get() * 2)
                            .expect("* 2 cannot make a number zero"),
                    )?
                    .processors()
                    .into_iter()
                    .cloned()
                    .chunks(2)
                    .into_iter()
                    .map(|pair| {
                        let mut pair = pair.collect_vec();
                        assert_eq!(pair.len(), 2);

                        let p1 = pair.pop().expect("we asserted there are two");
                        let p2 = pair.pop().expect("we asserted there are two");

                        let set1: ProcessorSet = p1.into();
                        let set2: ProcessorSet = p2.into();

                        (set1, set2)
                    })
                    .collect_vec(),
            )
        }
        WorkDistribution::PinnedSameProcessor => {
            // Here we do not care at all which processors are selected - work is
            // local on every processor, so just pick arbitrary processors and use
            // the same processor for both members of the pair.
            Some(
                candidates
                    .to_builder()
                    .take(worker_pair_count)?
                    .processors()
                    .into_iter()
                    .map(|p| {
                        let set1: ProcessorSet = p.clone().into();
                        let set2: ProcessorSet = p.clone().into();

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
            Some(first_processor_sets.into_iter().zip(partners).collect_vec())
        }
        WorkDistribution::UnpinnedSameMemoryRegion => {
            // We start by picking the first item in each pair. We still distribute the pairs
            // across all memory regions to even out the load and any hardware differences, even
            // though we do not actually care about crossing memory regions during operation.
            let first_processors = candidates
                .to_builder()
                .different_memory_regions()
                .take(worker_pair_count)?;

            // This must logically match our pair count because we have one pair per memory region
            // and expect to get one processor from each memory region with performance processors.
            assert_eq!(first_processors.len(), worker_pair_count.get());

            // Now we need to find a partner for each of the processors.
            let partners = first_processors
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

            // We now have pairs of processors from the same memory region. We need to expand the
            // sets to cover more, to implement the "unpinned" part of the distribution. However,
            // we also need to ensure that the processor sets in one pair are different because
            // any overlap would allow the platform to schedule the workers on the same processor,
            // which might distort results if the work is not compute-bound (such as giving a magic
            // speedup due to the processor's cache).
            let first_processors = first_processors.processors();
            let partner_processors = partners
                .into_iter()
                .map(|set| set.processors().first().clone());
            let pairs_single = first_processors
                .into_iter()
                .cloned()
                .zip(partner_processors)
                .collect_vec();

            Some(
                pairs_single
                    .into_iter()
                    .map(|(p1, p2)| {
                        let remaining = candidates
                            .to_builder()
                            .except([&p1, &p2])
                            .filter(|c| c.memory_region_id() == p1.memory_region_id())
                            .take_all();

                        // Without caring for how many there are, give half to each member of the pair.
                        match remaining {
                            None => {
                                // Nothing else left - that's fine.
                                (
                                    ProcessorSet::from_processors(nonempty![p1]),
                                    ProcessorSet::from_processors(nonempty![p2]),
                                )
                            }
                            Some(remaining) => {
                                let mut remaining_processors =
                                    remaining.processors().into_iter().cloned().collect_vec();
                                remaining_processors.shuffle(&mut rng());
                                let (p1_more, p2_more) =
                                    remaining_processors.split_at(remaining_processors.len() / 2);

                                (
                                    ProcessorSet::from_processors(
                                        NonEmpty::from_vec(
                                            [p1].into_iter()
                                                .chain(p1_more.iter().cloned())
                                                .collect_vec(),
                                        )
                                        .expect(
                                            "we know we have at least one processor for each pair",
                                        ),
                                    ),
                                    ProcessorSet::from_processors(
                                        NonEmpty::from_vec(
                                            [p2].into_iter()
                                                .chain(p2_more.iter().cloned())
                                                .collect_vec(),
                                        )
                                        .expect(
                                            "we know we have at least one processor for each pair",
                                        ),
                                    ),
                                )
                            }
                        }
                    })
                    .collect_vec(),
            )
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
    /// This is just useful if a single payload is a very small number (single digit milliseconds)
    /// and we cannot just increase the payload size because it is sized to fit into cache etc.
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
                | WorkDistribution::PinnedSameProcessor
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

                clean_caches();

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
