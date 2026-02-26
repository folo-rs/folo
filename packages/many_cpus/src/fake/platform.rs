//! Fake hardware backend implementation.

use std::fmt::{self, Display};
use std::sync::RwLock;
use std::thread::ThreadId;

use foldhash::{HashMap, HashMapExt};
use nonempty::NonEmpty;
use rand::prelude::*;
use rand::rng;

use crate::fake::HardwareBuilder;
use crate::pal::{AbstractProcessor, Platform, ProcessorFacade};
use crate::{EfficiencyClass, MemoryRegionId, ProcessorId};

/// A fake processor for use in fake hardware.
///
/// This is distinct from the test-only `FakeProcessor` in `pal/mocks.rs` because it needs
/// to be available when `test-util` feature is enabled, not just in test mode.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct FakeProcessor {
    id: ProcessorId,
    memory_region_id: MemoryRegionId,
    efficiency_class: EfficiencyClass,
}

impl Display for FakeProcessor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "FakeProcessor({} in region {}, {:?})",
            self.id, self.memory_region_id, self.efficiency_class
        )
    }
}

impl FakeProcessor {
    /// Creates a new fake processor.
    pub(crate) fn new(
        id: ProcessorId,
        memory_region_id: MemoryRegionId,
        efficiency_class: EfficiencyClass,
    ) -> Self {
        Self {
            id,
            memory_region_id,
            efficiency_class,
        }
    }
}

impl AbstractProcessor for FakeProcessor {
    fn id(&self) -> ProcessorId {
        self.id
    }

    fn memory_region_id(&self) -> MemoryRegionId {
        self.memory_region_id
    }

    fn efficiency_class(&self) -> EfficiencyClass {
        self.efficiency_class
    }
}

/// Per-thread state for fake hardware.
#[derive(Clone, Debug, Default)]
struct FakeThreadState {
    /// The processors this thread is allowed to run on.
    ///
    /// If `None`, all processors are allowed.
    allowed_processors: Option<NonEmpty<ProcessorId>>,
}

/// Fake hardware platform that simulates hardware configurations.
#[derive(Debug)]
pub(crate) struct FakePlatform {
    processors: Vec<FakeProcessor>,
    max_processor_id: ProcessorId,
    max_memory_region_id: MemoryRegionId,
    max_processor_time: f64,

    /// Per-thread state for tracking affinity.
    thread_states: RwLock<HashMap<ThreadId, FakeThreadState>>,

    /// Test-only override: when set, `current_processor_id()` returns this value
    /// instead of picking from configured processors.
    #[cfg(test)]
    processor_id_override: RwLock<Option<ProcessorId>>,
}

impl FakePlatform {
    /// Creates a new fake hardware backend from a builder.
    pub(crate) fn from_builder(builder: &HardwareBuilder) -> Self {
        let configured_processors = builder.build_processors();

        assert!(
            !configured_processors.is_empty(),
            "at least one processor must be configured"
        );

        let processors: Vec<FakeProcessor> = configured_processors
            .iter()
            .map(|p| FakeProcessor::new(p.id, p.memory_region_id, p.efficiency_class))
            .collect();

        let max_processor_id = processors.iter().map(|p| p.id).max().unwrap_or(0);

        let max_memory_region_id = processors
            .iter()
            .map(|p| p.memory_region_id)
            .max()
            .unwrap_or(0);

        // Default max processor time to the number of processors (no quota).
        // A typical machine has at most a few hundred processors, well within f64 precision.
        #[expect(
            clippy::cast_precision_loss,
            reason = "processor count is small enough for precise f64 representation"
        )]
        let max_processor_time = builder
            .build_max_processor_time()
            .unwrap_or(processors.len() as f64);

        Self {
            processors,
            max_processor_id,
            max_memory_region_id,
            max_processor_time,
            thread_states: RwLock::new(HashMap::new()),
            #[cfg(test)]
            processor_id_override: RwLock::new(None),
        }
    }

    /// Gets the "current processor ID" for a thread using random selection.
    ///
    /// This simulates real-world behavior where unpinned threads can move between processors.
    /// Each call may return a different processor from the allowed set.
    fn thread_processor_id(&self, thread_id: ThreadId) -> ProcessorId {
        // First check if the thread has a restricted set of processors.
        let allowed = {
            let states = self
                .thread_states
                .read()
                .expect("thread state lock should never be poisoned");
            states
                .get(&thread_id)
                .and_then(|s| s.allowed_processors.clone())
        };

        match allowed {
            Some(processors) => {
                // Pick a random processor from the allowed set.
                *processors
                    .iter()
                    .choose(&mut rng())
                    .expect("allowed processors is non-empty")
            }
            None => {
                // Pick a random processor from all processors.
                self.processors
                    .iter()
                    .choose(&mut rng())
                    .expect("at least one processor was configured")
                    .id
            }
        }
    }

    /// Sets an override so that `current_processor_id()` returns the given value
    /// regardless of thread affinity. This allows testing fallback paths when the
    /// returned processor ID does not correspond to a configured processor.
    #[cfg(test)]
    pub(crate) fn set_processor_id_override(&self, id: Option<ProcessorId>) {
        *self
            .processor_id_override
            .write()
            .expect("processor_id_override lock should never be poisoned") = id;
    }
}

impl Platform for FakePlatform {
    fn get_all_processors(&self) -> NonEmpty<ProcessorFacade> {
        let facades: Vec<ProcessorFacade> = self
            .processors
            .iter()
            .map(|p| ProcessorFacade::Fake(*p))
            .collect();

        NonEmpty::from_vec(facades).expect("at least one processor was configured")
    }

    fn pin_current_thread_to<P>(&self, processors: &NonEmpty<P>)
    where
        P: AsRef<ProcessorFacade>,
    {
        let thread_id = std::thread::current().id();
        let processor_ids: Vec<ProcessorId> = processors.iter().map(|p| p.as_ref().id()).collect();
        let processor_ids = NonEmpty::from_vec(processor_ids)
            .expect("processors is non-empty so the resulting vec is also non-empty");

        let mut states = self
            .thread_states
            .write()
            .expect("thread state lock should never be poisoned");

        let state = states.entry(thread_id).or_default();
        state.allowed_processors = Some(processor_ids);
    }

    fn current_processor_id(&self) -> ProcessorId {
        #[cfg(test)]
        if let Some(id) = *self
            .processor_id_override
            .read()
            .unwrap()
        {
            return id;
        }

        let thread_id = std::thread::current().id();
        self.thread_processor_id(thread_id)
    }

    fn current_thread_processors(&self) -> NonEmpty<ProcessorId> {
        let thread_id = std::thread::current().id();
        let states = self
            .thread_states
            .read()
            .expect("thread state lock should never be poisoned");

        match states
            .get(&thread_id)
            .and_then(|s| s.allowed_processors.clone())
        {
            Some(processors) => processors,
            None => {
                // Return all processor IDs.
                let all_ids: Vec<ProcessorId> = self.processors.iter().map(|p| p.id).collect();
                NonEmpty::from_vec(all_ids).expect("at least one processor was configured")
            }
        }
    }

    fn max_processor_id(&self) -> ProcessorId {
        self.max_processor_id
    }

    fn max_memory_region_id(&self) -> MemoryRegionId {
        self.max_memory_region_id
    }

    fn max_processor_time(&self) -> f64 {
        self.max_processor_time
    }

    fn active_processor_count(&self) -> usize {
        self.processors.len()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::indexing_slicing, reason = "test code, panics are acceptable")]
mod tests {
    use new_zealand::nz;

    use super::*;
    use crate::fake::ProcessorBuilder;

    #[test]
    fn basic_hardware_creation() {
        let builder = HardwareBuilder::from_counts(nz!(4), nz!(1));
        let backend = FakePlatform::from_builder(&builder);

        assert_eq!(backend.processors.len(), 4);
        assert_eq!(backend.max_processor_id, 3);
        assert_eq!(backend.max_memory_region_id, 0);
        assert_eq!(backend.active_processor_count(), 4);
    }

    #[test]
    fn custom_memory_regions() {
        let builder = HardwareBuilder::from_counts(nz!(4), nz!(2));
        let backend = FakePlatform::from_builder(&builder);

        // Processors should be distributed round-robin.
        assert_eq!(backend.processors[0].memory_region_id, 0);
        assert_eq!(backend.processors[1].memory_region_id, 1);
        assert_eq!(backend.processors[2].memory_region_id, 0);
        assert_eq!(backend.processors[3].memory_region_id, 1);
        assert_eq!(backend.max_memory_region_id, 1);
    }

    #[test]
    fn custom_processor_configuration() {
        let builder = HardwareBuilder::new()
            .processor(
                ProcessorBuilder::new()
                    .id(0)
                    .memory_region(0)
                    .efficiency_class(EfficiencyClass::Performance),
            )
            .processor(
                ProcessorBuilder::new()
                    .id(1)
                    .memory_region(1)
                    .efficiency_class(EfficiencyClass::Efficiency),
            );
        let backend = FakePlatform::from_builder(&builder);

        assert_eq!(backend.processors.len(), 2);
        assert_eq!(
            backend.processors[0].efficiency_class,
            EfficiencyClass::Performance
        );
        assert_eq!(
            backend.processors[1].efficiency_class,
            EfficiencyClass::Efficiency
        );
        assert_eq!(backend.max_memory_region_id, 1);
    }

    #[test]
    fn max_processor_time_configuration() {
        let builder = HardwareBuilder::from_counts(nz!(4), nz!(1)).max_processor_time(2.5);
        let backend = FakePlatform::from_builder(&builder);

        assert!((backend.max_processor_time - 2.5).abs() < f64::EPSILON);
    }

    #[test]
    fn current_processor_id_is_from_valid_set() {
        let builder = HardwareBuilder::from_counts(nz!(4), nz!(1));
        let backend = FakePlatform::from_builder(&builder);

        // Processor ID should be from the set of configured processors.
        let id = backend.current_processor_id();

        assert!(id < 4);
    }

    #[test]
    fn get_all_processors_returns_all() {
        let builder = HardwareBuilder::from_counts(nz!(4), nz!(1));
        let backend = FakePlatform::from_builder(&builder);

        let processors = backend.get_all_processors();

        assert_eq!(processors.len(), 4);
    }

    #[test]
    fn pin_current_thread_to_restricts_processor_selection() {
        let builder = HardwareBuilder::from_counts(nz!(4), nz!(1));
        let backend = FakePlatform::from_builder(&builder);

        // Create a NonEmpty with a single processor facade.
        let processor = backend.get_all_processors().head;
        let processors = NonEmpty::singleton(processor);

        backend.pin_current_thread_to(&processors);

        // After pinning, current_processor_id should return the pinned processor.
        let id = backend.current_processor_id();
        assert_eq!(id, processor.id());
    }

    #[test]
    fn current_thread_processors_returns_all_when_not_pinned() {
        let builder = HardwareBuilder::from_counts(nz!(4), nz!(1));
        let backend = FakePlatform::from_builder(&builder);

        let processors = backend.current_thread_processors();

        assert_eq!(processors.len(), 4);
    }

    #[test]
    fn current_thread_processors_returns_pinned_set() {
        let builder = HardwareBuilder::from_counts(nz!(4), nz!(1));
        let backend = FakePlatform::from_builder(&builder);

        // Pin to processor 0 and 1.
        let all = backend.get_all_processors();
        let mut iter = all.iter();
        let p0 = *iter.next().unwrap();
        let p1 = *iter.next().unwrap();
        let pinned = NonEmpty::from((p0, vec![p1]));

        backend.pin_current_thread_to(&pinned);

        let thread_processors = backend.current_thread_processors();

        assert_eq!(thread_processors.len(), 2);
        assert!(thread_processors.iter().any(|&id| id == p0.id()));
        assert!(thread_processors.iter().any(|&id| id == p1.id()));
    }

    #[test]
    fn thread_processor_id_selects_from_pinned_set() {
        let builder = HardwareBuilder::from_counts(nz!(8), nz!(1));
        let backend = FakePlatform::from_builder(&builder);

        // Pin to processors 2 and 3 only.
        let all = backend.get_all_processors();
        let processors_vec: Vec<_> = all.iter().copied().collect();
        let p2 = processors_vec[2];
        let p3 = processors_vec[3];
        let pinned = NonEmpty::from((p2, vec![p3]));

        backend.pin_current_thread_to(&pinned);

        let id = backend.current_processor_id();

        // The returned ID must be one of the pinned processors.
        assert!(id == p2.id() || id == p3.id());
    }

    #[test]
    fn fake_processor_display_includes_all_fields() {
        let processor = FakeProcessor::new(5, 2, EfficiencyClass::Efficiency);

        let display = format!("{processor}");

        assert!(display.contains('5'));
        assert!(display.contains('2'));
        assert!(display.contains("Efficiency"));
    }

    #[test]
    fn fake_processor_efficiency_class_returns_configured_value() {
        let perf = FakeProcessor::new(0, 0, EfficiencyClass::Performance);
        let eff = FakeProcessor::new(1, 0, EfficiencyClass::Efficiency);

        assert_eq!(perf.efficiency_class(), EfficiencyClass::Performance);
        assert_eq!(eff.efficiency_class(), EfficiencyClass::Efficiency);
    }

    #[test]
    fn fake_processor_id_returns_configured_value() {
        let processor = FakeProcessor::new(42, 0, EfficiencyClass::Performance);

        assert_eq!(processor.id(), 42);
    }

    #[test]
    fn fake_processor_memory_region_id_returns_configured_value() {
        let processor = FakeProcessor::new(0, 7, EfficiencyClass::Performance);

        assert_eq!(processor.memory_region_id(), 7);
    }
}
