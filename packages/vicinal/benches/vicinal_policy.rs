#![expect(missing_docs, reason = "benchmarks")]

use std::hint::black_box;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus::SystemHardware;
use many_cpus::fake::{HardwareBuilder, ProcessorBuilder};
use new_zealand::nz;
use vicinal::policies::{WorkersPerMemoryRegion, WorkersPerProcessor, WorkersPerSystem};
use vicinal::{
    BoundWorkerPolicy, Pool, PrepareContext, PrepareError, WorkerGroupId, WorkerGroups,
    WorkerPolicy,
};

::testing::set_allocator!();

/// Built-in policies with distinct preparation and established-selection paths.
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

/// Retains prepared public policy state after `PoolBuilder` moves one clone into the pool.
#[derive(Clone, Debug)]
struct PolicyProbe {
    kind: PolicyCase,
    state: Arc<OnceLock<(WorkerGroups, Box<dyn BoundWorkerPolicy>)>>,
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
            .expect("PoolBuilder prepares the retained policy");
        policy.select_group(groups)
    }
}

impl WorkerPolicy for PolicyProbe {
    fn bind(self, context: PrepareContext<'_>) -> Result<impl BoundWorkerPolicy, PrepareError> {
        let policy: Box<dyn BoundWorkerPolicy> = match self.kind {
            PolicyCase::Processor => Box::new(WorkersPerProcessor::new(nz!(1)).bind(context)?),
            PolicyCase::MemoryRegion => {
                Box::new(WorkersPerMemoryRegion::new(nz!(1)).bind(context)?)
            }
            PolicyCase::System => Box::new(WorkersPerSystem::new(nz!(1)).bind(context)?),
        };
        assert!(
            self.state.set((context.groups().clone(), policy)).is_ok(),
            "the benchmark binds each policy once"
        );
        Ok(self)
    }
}

impl BoundWorkerPolicy for PolicyProbe {
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
struct NoWorkerPolicy {
    group: OnceLock<WorkerGroupId>,
}

impl WorkerPolicy for NoWorkerPolicy {
    fn bind(self, context: PrepareContext<'_>) -> Result<impl BoundWorkerPolicy, PrepareError> {
        assert!(
            self.group.set(context.groups().add().id()).is_ok(),
            "the benchmark binds the no-worker policy once"
        );
        Ok(self)
    }
}

impl BoundWorkerPolicy for NoWorkerPolicy {
    fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
        *self
            .group
            .get()
            .expect("PoolBuilder binds the no-worker policy")
    }
}

/// Supplies stable fake topology without OS hardware discovery or affinity syscalls.
///
/// Processors 0 and 1 intentionally share a region so the membership benchmark expands an
/// established regional route instead of starting an unused route.
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

fn preparation(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("vicinal_policy/preparation");
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

fn admission(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("vicinal_policy/admission");

    group.bench_function("joined", |b| {
        b.iter_custom(|iterations| {
            let mut elapsed = Duration::ZERO;
            for _ in 0..iterations {
                let pool = Pool::builder()
                    .hardware(hardware())
                    .worker_policy(NoWorkerPolicy {
                        group: OnceLock::new(),
                    })
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
                    .worker_policy(NoWorkerPolicy {
                        group: OnceLock::new(),
                    })
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
    preparation(criterion);
    established_selection(criterion);
    membership_expansion(criterion);
    admission(criterion);
}

criterion_group!(benches, entrypoint);
criterion_main!(benches);
