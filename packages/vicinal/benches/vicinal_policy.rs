#![expect(
    missing_docs,
    reason = "this benchmark executable exposes no consumer-facing API to document"
)]

use std::hint::black_box;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus::SystemHardware;
use many_cpus::fake::{HardwareBuilder, ProcessorBuilder};
use new_zealand::nz;
use vicinal::Pool;
use vicinal::policies::{WorkersPerMemoryRegion, WorkersPerProcessor, WorkersPerSystem};
use vicinal::worker_policy::{
    BindContext, BindError, BoundPolicy, WorkerGroupId, WorkerGroups, WorkerPolicy,
};

::testing::set_allocator!();

/// Built-in policies with distinct binding and established-selection paths.
#[derive(Clone, Copy, Debug)]
enum PolicyCase {
    Processor,
    MemoryRegion,
    System,
}

impl PolicyCase {
    fn name(self) -> &'static str {
        match self {
            Self::Processor => "processor",
            Self::MemoryRegion => "memory_region",
            Self::System => "system",
        }
    }
}

/// Retains bound public policy state after `PoolBuilder` moves one clone into the pool.
#[derive(Clone, Debug)]
struct PolicyProbe {
    kind: PolicyCase,
    // Clones share a single binding. Retain its registry so direct selection uses the same
    // registry that bound the policy, without going through the scheduler.
    state: Arc<OnceLock<(WorkerGroups, Box<dyn BoundPolicy>)>>,
}

impl PolicyProbe {
    fn new(kind: PolicyCase) -> Self {
        Self {
            kind,
            state: Arc::new(OnceLock::new()),
        }
    }

    fn select(&self) -> WorkerGroupId {
        let (groups, policy) = self
            .state
            .get()
            .expect("PoolBuilder binds the retained policy");
        policy.select_group(groups)
    }
}

impl WorkerPolicy for PolicyProbe {
    fn bind(self, context: BindContext<'_>) -> Result<impl BoundPolicy, BindError> {
        // Use the smallest supported worker count for every policy to control multiplicity
        // and limit incidental startup work.
        let workers = nz!(1);
        let policy: Box<dyn BoundPolicy> = match self.kind {
            PolicyCase::Processor => Box::new(WorkersPerProcessor::new(workers).bind(context)?),
            PolicyCase::MemoryRegion => {
                Box::new(WorkersPerMemoryRegion::new(workers).bind(context)?)
            }
            PolicyCase::System => Box::new(WorkersPerSystem::new(workers).bind(context)?),
        };
        assert!(
            self.state.set((context.groups().clone(), policy)).is_ok(),
            "the benchmark binds each policy once"
        );
        Ok(self)
    }
}

impl BoundPolicy for PolicyProbe {
    fn select_group(&self, groups: &WorkerGroups) -> WorkerGroupId {
        self.state
            .get()
            .expect("PoolBuilder binds the retained policy")
            .1
            .select_group(groups)
    }
}

/// Prepares one stable group without creating workers, isolating scheduler admission.
#[derive(Debug)]
struct NoWorkerPolicy;

impl WorkerPolicy for NoWorkerPolicy {
    fn bind(self, context: BindContext<'_>) -> Result<impl BoundPolicy, BindError> {
        Ok(BoundNoWorkerPolicy {
            group: context.groups().add().id(),
        })
    }
}

/// Routes admission benchmarks to the group prepared during binding.
#[derive(Debug)]
struct BoundNoWorkerPolicy {
    group: WorkerGroupId,
}

impl BoundPolicy for BoundNoWorkerPolicy {
    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        self.group
    }
}

/// Supplies stable fake topology without OS hardware discovery or affinity syscalls.
///
/// The processors used for membership expansion share a region so the second selection
/// expands an established route rather than creating a new one.
fn hardware() -> SystemHardware {
    SystemHardware::fake(
        HardwareBuilder::new()
            .processor(ProcessorBuilder::new().id(0).memory_region(2))
            .processor(ProcessorBuilder::new().id(1).memory_region(2))
            .processor(ProcessorBuilder::new().id(2).memory_region(5)),
    )
}

/// Switches the fake hardware's current processor entirely in-process.
fn pin_to(hardware: &SystemHardware, processor: u32) {
    hardware
        .all_processors()
        .to_builder()
        .filter(|candidate| candidate.id() == processor)
        .take_all()
        .unwrap()
        .pin_current_thread_to();
}

/// Times pool construction from builder configuration through `build()` returning.
///
/// Probe creation, hardware setup, and shutdown stay outside measurement.
fn binding(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("vicinal_policy/binding");
    let hardware = hardware();

    for kind in [
        PolicyCase::Processor,
        PolicyCase::MemoryRegion,
        PolicyCase::System,
    ] {
        group.bench_function(kind.name(), |b| {
            b.iter_custom(|iterations| {
                let mut operation_elapsed = Duration::ZERO;
                for _ in 0..iterations {
                    let probe = PolicyProbe::new(kind);
                    let hardware = hardware.clone();
                    let start = Instant::now();
                    let pool = Pool::builder()
                        .hardware(hardware)
                        .worker_policy(probe.clone())
                        .build()
                        .unwrap();
                    operation_elapsed = operation_elapsed.saturating_add(start.elapsed());
                    black_box(&probe);
                    black_box(&pool);
                    drop(pool);
                }
                operation_elapsed
            });
        });
    }

    group.finish();
}

/// Times a direct policy selection on a prepared route, excluding pool setup, initial
/// selection, scheduler admission, and shutdown.
fn established_selection(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("vicinal_policy/selection");

    for kind in [
        PolicyCase::Processor,
        PolicyCase::MemoryRegion,
        PolicyCase::System,
    ] {
        group.bench_function(kind.name(), |b| {
            let hardware = hardware();
            pin_to(&hardware, 0);
            let probe = PolicyProbe::new(kind);
            let pool = Pool::builder()
                .hardware(hardware)
                .worker_policy(probe.clone())
                .build()
                .unwrap();
            black_box(probe.select());

            b.iter(|| black_box(probe.select()));
            drop(pool);
        });
    }

    group.finish();
}

/// Times the second direct selection, including publication of the expanded membership.
/// Initial selection, pool shutdown, and switching processors happen before measurement.
fn membership_expansion(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("vicinal_policy/membership");

    for kind in [PolicyCase::MemoryRegion, PolicyCase::System] {
        group.bench_function(kind.name(), |b| {
            b.iter_custom(|iterations| {
                let mut elapsed = Duration::ZERO;
                for _ in 0..iterations {
                    let hardware = hardware();
                    pin_to(&hardware, 0);
                    let probe = PolicyProbe::new(kind);
                    let pool = Pool::builder()
                        .hardware(hardware.clone())
                        .worker_policy(probe.clone())
                        .build()
                        .unwrap();
                    black_box(probe.select());
                    // Stop the pool during setup so no worker remains participating. The
                    // measured expansion then only constructs and publishes the new mask;
                    // no worker wakeup can enter it.
                    drop(pool);
                    pin_to(&hardware, 1);

                    let start = Instant::now();
                    black_box(probe.select());
                    elapsed = elapsed.saturating_add(start.elapsed());
                }
                elapsed
            });
        });
    }

    group.finish();
}

/// Times submission through return from the scheduler.
///
/// The pool has no workers, so no task runs. Setup and shutdown stay outside measurement.
fn admission(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("vicinal_policy/admission");

    group.bench_function("joined", |b| {
        b.iter_custom(|iterations| {
            let mut elapsed = Duration::ZERO;
            for _ in 0..iterations {
                let pool = Pool::builder()
                    .hardware(hardware())
                    .worker_policy(NoWorkerPolicy)
                    .build()
                    .unwrap();
                let scheduler = pool.scheduler();
                let start = Instant::now();
                let handle = scheduler.spawn(|| black_box(0));
                elapsed = elapsed.saturating_add(start.elapsed());
                black_box(&handle);
                drop(pool);
                drop(handle);
            }
            elapsed
        });
    });

    group.bench_function("fire_and_forget", |b| {
        b.iter_custom(|iterations| {
            let mut elapsed = Duration::ZERO;
            for _ in 0..iterations {
                let pool = Pool::builder()
                    .hardware(hardware())
                    .worker_policy(NoWorkerPolicy)
                    .build()
                    .unwrap();
                let scheduler = pool.scheduler();
                let start = Instant::now();
                scheduler.spawn_and_forget(|| {
                    black_box(());
                });
                elapsed = elapsed.saturating_add(start.elapsed());
                black_box(&scheduler);
                drop(pool);
            }
            elapsed
        });
    });

    group.finish();
}

fn entrypoint(criterion: &mut Criterion) {
    binding(criterion);
    established_selection(criterion);
    membership_expansion(criterion);
    admission(criterion);
}

criterion_group!(benches, entrypoint);
criterion_main!(benches);
