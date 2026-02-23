use std::any::type_name;
use std::collections::VecDeque;
use std::iter::{once, repeat_with};
use std::num::NonZero;
use std::sync::{Arc, Barrier, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use std::{env, mem};

use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion, SamplingMode};
use itertools::Itertools;
use many_cpus::{Processor, ProcessorSet, SystemHardware};
use new_zealand::nz;
use nonempty::{NonEmpty, nonempty};
use rand::rng;
use rand::seq::SliceRandom;

use crate::{Payload, WorkDistribution};

/// Executes a number of benchmark runs for a specific payload type, using the specified work
/// distribution modes.
///
/// `BATCH_SIZE` indicates the maximum number of iterations that can be prepared at the same time.
/// It is optimal to prepare all iterations at once, to get the most consistent and noise-free
/// results. However, there is only a limited amount of memory available, making that impractical.
/// Assign the highest value that is low enough for this many payloads to fit in memory at the
/// same time on every worker thread. The benchmark infrastructure may also limit this value, so
/// this is merely an upper bound.
pub fn execute_runs<P: Payload, const BATCH_SIZE: u64>(
    c: &mut Criterion,
    work_distributions: &[WorkDistribution],
) {
    let mut g = c.benchmark_group(type_name::<P>());

    // Many-processor benchmarks can be slow and clearing processor caches adds extra overhead
    // between iterations, so to get stable and consistent data it is worth taking some time.
    g.measurement_time(Duration::from_secs(30));

    // Criterion docs say that this is faster for slow benchmarks (which ours definitely are).
    // The downside is that it supposedly disabled some advanced statistical analysis but that
    // is not really something we care about here - we just want the basic duration scoring.
    g.sampling_mode(SamplingMode::Flat);

    for &distribution in work_distributions {
        execute_run::<P, BATCH_SIZE>(&mut g, distribution);
    }

    g.finish();
}

/// In some execution modes, we are only executing to list the benchmarks or to perform a dummy
/// run of a single iteration to test that it works. In these cases, we do not want to emit any
/// additional output to stderr because it will confuse the test runner.
fn is_fake_run() -> bool {
    // --test is used by cargo test
    // --exact is used by nextest
    // --list is used by both
    env::args().any(|a| a == "--test" || a == "--list" || a == "--exact")
}

fn execute_run<P: Payload, const BATCH_SIZE: u64>(
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
            let cpulist1 = cpulist::emit(processor_set_1.processors().iter().map(Processor::id));
            let cpulist2 = cpulist::emit(processor_set_2.processors().iter().map(Processor::id));

            eprintln!("{work_distribution} reference selection: ({cpulist1}) & ({cpulist2})");
        }
    }

    g.bench_function(work_distribution.to_string(), |b| {
        b.iter_custom(move |iters| {
            let mut total_duration = Duration::ZERO;

            let mut iters_remaining = iters;

            while iters_remaining > 0 {
                let batch_size = iters_remaining.min(BATCH_SIZE);

                iters_remaining = iters_remaining
                    .checked_sub(batch_size)
                    .expect("we used min() above to ensure we do not consume more iterations than remaining");

                // Each batch uses the same selection of processors.
                let processor_set_pairs = get_processor_set_pairs(work_distribution)
                    .expect("we already validated that we have the right topology");

                let batch_duration = BenchmarkBatch::new::<P>(&processor_set_pairs, work_distribution, batch_size)
                    .wait();

                total_duration = total_duration.checked_add(batch_duration)
                    .expect("duration overflow is unfathomable within our spacetime boundaries");
            }

            total_duration
        });
    });
}

/// Identifies how many worker thread pairs we need to use in the benchmark, based on the hardware
/// topology of the system, using the "pinned memory region pairs" reference scenario.
///
/// All work distributions use the same number of pairs as the reference scenario, for
/// optimal comparability between different distributions.
fn calculate_worker_pair_count() -> NonZero<usize> {
    // One pair for every memory region. That is it.
    NonZero::new(
        SystemHardware::current()
            .processors()
            .to_builder()
            .performance_processors_only()
            .take_all()
            .expect("must have at least one processor")
            .processors()
            .iter()
            .map(Processor::memory_region_id)
            .unique()
            .count(),
    )
    .expect("there must be at least one memory region")
}

const ONE_PROCESSOR: NonZero<usize> = nz!(1);

/// Obtains the processor pairs to use for one iteration of the benchmark. We pick different
/// processors for different iterations to help average out any differences in performance
/// that may exist due to competing workloads.
///
/// For the "pinned" variants, this returns for each pair of workers a pair of single-processor
/// `ProcessorSet`s. For the "unpinned" variants, this returns for each pair of workers a pair of
/// many-processor `ProcessorSet`s.
fn get_processor_set_pairs(
    distribution: WorkDistribution,
) -> Option<Vec<(ProcessorSet, ProcessorSet)>> {
    let worker_pair_count = calculate_worker_pair_count();

    // If the system has efficiency processors, we do not want them.
    let candidates = SystemHardware::current()
        .processors()
        .to_builder()
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
                        let set1 = candidates.to_builder().take_exact(nonempty![p1]);
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
                        let set1 = candidates.to_builder().take_exact(nonempty![p1]);
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
                        NonZero::new(
                            worker_pair_count
                                .get()
                                .checked_mul(2)
                                .expect("no system will ever have that many processors"),
                        )
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

                        let set1 = candidates.to_builder().take_exact(nonempty![p1]);
                        let set2 = candidates.to_builder().take_exact(nonempty![p2]);

                        (set1, set2)
                    })
                    .collect_vec(),
            )
        }
        WorkDistribution::PinnedSameProcessor => {
            // To maintain comparability between distributions and avoid structural randomness,
            // we pick one processor from each NUMA node - the same logic as with region-pairs.
            // We start by picking the first item in each pair.
            let processors = candidates
                .to_builder()
                .different_memory_regions()
                .take(worker_pair_count)?;

            // This must logically match our pair count because we have one pair per memory region
            // and expect to get one processor from each memory region with performance processors.
            assert_eq!(processors.len(), worker_pair_count.get());

            Some(
                processors
                    .processors()
                    .into_iter()
                    .map(|p| {
                        let set1 = candidates.to_builder().take_exact(nonempty![p.clone()]);
                        let set2 = candidates.to_builder().take_exact(nonempty![p.clone()]);

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
            // In principle, this does allow multiple workers to be scheduled on the same
            // processor, but that is not a problem for our purposes because those two workers
            // are not working together - they are paired with different workers each, with
            // different payloads each. At most, this can degrade performance but the OS will
            // likely schedule them on different processors anyway since it sees this.
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
        WorkDistribution::ConstrainedSameMemoryRegion => {
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
                                // Nothing else left - that is fine.
                                (
                                    candidates.to_builder().take_exact(nonempty![p1]),
                                    candidates.to_builder().take_exact(nonempty![p2]),
                                )
                            }
                            Some(remaining) => {
                                let mut remaining_processors =
                                    remaining.processors().into_iter().cloned().collect_vec();
                                remaining_processors.shuffle(&mut rng());

                                #[expect(
                                    clippy::integer_division,
                                    reason = "we do not care if one side has one more than the other - what matters is lack of overlap"
                                )]
                                let (p1_more, p2_more) =
                                    remaining_processors.split_at(remaining_processors.len() / 2);

                                (
                                    candidates.to_builder().take_exact(
                                        NonEmpty::from_vec(
                                            once(p1).chain(p1_more.iter().cloned()).collect_vec(),
                                        )
                                        .expect(
                                            "we know we have at least one processor for each pair",
                                        ),
                                    ),
                                    candidates.to_builder().take_exact(
                                        NonEmpty::from_vec(
                                            once(p2).chain(p2_more.iter().cloned()).collect_vec(),
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
                repeat_with(|| (candidates.clone(), candidates.clone()))
                    .take(worker_pair_count.get())
                    .collect(),
            )
        }
        WorkDistribution::UnpinnedPerMemoryRegionSelf => {
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

            // We now expand each single processor into "all the processors in that memory region".
            // In principle, this does allow multiple workers to be scheduled on the same
            // processor, but that is not a problem for our purposes because those two workers
            // are not working together - they are each processing their own separate payload.
            // At most, this can degrade performance but the OS will likely schedule them on
            // different processors anyway since it sees this.
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

            // We pair them up with themselves, so we need to clone the whole thing.
            let partners = first_processor_sets.clone();

            Some(first_processor_sets.into_iter().zip(partners).collect_vec())
        }
    }
}

#[derive(Debug)]
struct BenchmarkBatch {
    join_handles: Box<[JoinHandle<Duration>]>,
}

impl BenchmarkBatch {
    fn new<P: Payload>(
        processor_set_pairs: &[(ProcessorSet, ProcessorSet)],
        distribution: WorkDistribution,
        batch_size: u64,
    ) -> Self {
        assert_ne!(processor_set_pairs.len(), 0);

        let batch_size = usize::try_from(batch_size)
            .expect("batch_size greater than usize::MAX is not going to work out - we cannot feasibly prepare that many payloads in a single batch");

        // Once by current thread + once by each worker in each pair.
        // All workers will start when all workers and the main thread are ready.
        let worker_count = processor_set_pairs
            .len()
            .checked_mul(2)
            .expect("we will never have so many processors that we overflow usize");

        let workers_plus_coordinator = worker_count.checked_add(1).expect(
            "we will never have so many processors that we overflow usize, even if we add one",
        );

        let ready_signal = Arc::new(Barrier::new(workers_plus_coordinator));

        let mut join_handles = Vec::with_capacity(worker_count);

        for processor_set_pair in processor_set_pairs {
            let (processor_set_1, processor_set_2) = processor_set_pair;

            // We generate the payload instances here (but do not prepare them yet).
            let (payloads1, payloads2) = repeat_with(|| P::new_pair()).take(batch_size).unzip();

            // Each payload is also associated with a Barrier that will synchronize when that
            // payload is allowed to start executing, to ensure that any impact from multithreaded
            // access is felt at the same time. Time spent waiting for the barrier is still counted
            // as part of the benchmark duration because we are mainly interested in the relative
            // durations of different configurations and not the "pure" absolute timings.
            let payload_barriers = repeat_with(|| Arc::new(Barrier::new(2)))
                .take(batch_size)
                .collect_vec();

            // We use these to deliver a prepared payload to the worker meant to process it.
            // Depending on the mode, we either wire up the channels to themselves or each other.
            let (c1_tx, c1_rx) = mpsc::channel::<Vec<P>>();
            let (c2_tx, c2_rx) = mpsc::channel::<Vec<P>>();

            let ((tx1, rx1), (tx2, rx2)) = match distribution {
                WorkDistribution::PinnedMemoryRegionPairs
                | WorkDistribution::PinnedSameMemoryRegion
                | WorkDistribution::PinnedSameProcessor
                | WorkDistribution::UnpinnedMemoryRegionPairs
                | WorkDistribution::ConstrainedSameMemoryRegion => {
                    // Worker 1 will send to worker 2 and vice versa.
                    ((c1_tx, c2_rx), (c2_tx, c1_rx))
                }
                WorkDistribution::PinnedSelf
                | WorkDistribution::UnpinnedSelf
                | WorkDistribution::UnpinnedPerMemoryRegionSelf => {
                    // Worker 1 will send to itself and worker 2 will send to itself.
                    ((c1_tx, c1_rx), (c2_tx, c2_rx))
                }
            };

            // We stuff everything in a bag. Whichever thread starts first takes the first group.
            let bag = Arc::new(Mutex::new(vec![
                (tx1, rx1, payloads1, payload_barriers.clone()),
                (tx2, rx2, payloads2, payload_barriers),
            ]));

            join_handles.push(Self::spawn_worker(
                processor_set_1,
                Arc::clone(&ready_signal),
                Arc::clone(&bag),
            ));
            join_handles.push(Self::spawn_worker(
                processor_set_2,
                Arc::clone(&ready_signal),
                Arc::clone(&bag),
            ));
        }

        ready_signal.wait();

        Self {
            join_handles: join_handles.into_boxed_slice(),
        }
    }

    fn wait(&mut self) -> Duration {
        let thread_count = self.join_handles.len();

        let join_handles = mem::replace(&mut self.join_handles, Box::new([]));

        let mut total_elapsed_nanos: u128 = 0;

        for thread in join_handles {
            let elapsed = thread.join().unwrap();
            total_elapsed_nanos = total_elapsed_nanos
                .checked_add(elapsed.as_nanos())
                .expect("elapsed time overflow is unfathomable within our spacetime boundaries");
        }

        // We return the average duration of all threads as the duration of the iteration.
        let total_elapsed_nanos_per_thread = total_elapsed_nanos
            .checked_div(thread_count as u128)
            .expect(
                "thread count is asserted as non-zero in ctor, so division by zero is impossible",
            );

        Duration::from_nanos(
            total_elapsed_nanos_per_thread
                .try_into()
                .expect("duration overflow is unfathomable within our spacetime boundaries"),
        )
    }

    #[expect(
        clippy::type_complexity,
        reason = "only used once, so we accept it as cost of doing business"
    )]
    fn spawn_worker<P: Payload>(
        processor_set: &ProcessorSet,
        ready_signal: Arc<Barrier>,
        payload_bag: Arc<
            Mutex<
                Vec<(
                    mpsc::Sender<Vec<P>>,
                    mpsc::Receiver<Vec<P>>,
                    Vec<P>,
                    Vec<Arc<Barrier>>,
                )>,
            >,
        >,
    ) -> JoinHandle<Duration> {
        processor_set.spawn_thread({
            move |_| {
                let (payloads_tx, payloads_rx, mut payloads, mut payload_barriers) =
                    payload_bag.lock().unwrap().pop().unwrap();

                for payload in &mut payloads {
                    payload.prepare();
                }

                // Potentially trade payloads with the other worker in the pair.
                // This may or may not go anywhere - it might just send back to itself.
                payloads_tx.send(payloads).unwrap();
                let mut payloads = payloads_rx.recv().unwrap();

                // We skip this when in debug/test builds because it is expensive - this is only
                // important for real benchmarks, not testing (we do test it separately, of course).
                #[cfg(all(not(test), not(debug_assertions)))]
                crate::cache::clean_caches();

                // This signal is set when all workers have completed the "prepare" step.
                ready_signal.wait();

                let mut total_duration = Duration::ZERO;

                for payload in &mut payloads {
                    // We need to synchronize with other workers before starting on each payload
                    // because we want each worker to access the same payload at the same time
                    // to see any multithreading related effects.
                    payload_barriers
                        .pop()
                        .expect("caller gave us wrong number of barriers?!")
                        .wait();

                    payload.prepare_local();

                    let start = Instant::now();

                    payload.process();

                    let elapsed = start.elapsed();
                    total_duration = total_duration.checked_add(elapsed).expect(
                        "duration overflow is unfathomable within our spacetime boundaries",
                    );
                }

                // The payloads are dropped at the end, ensuring that we do not accidentally
                // measure any of the "drop" overhead above, during the benchmark iterations.
                drop(payloads);

                total_duration
            }
        })
    }
}

impl Drop for BenchmarkBatch {
    fn drop(&mut self) {
        let join_handles = mem::replace(&mut self.join_handles, Box::new([]));

        for handle in join_handles {
            // In case something panicked and we left the panic hanging, this will bring it to
            // the main thread. Generally, wait() should already consume all the threads.
            handle.join().unwrap();
        }
    }
}
