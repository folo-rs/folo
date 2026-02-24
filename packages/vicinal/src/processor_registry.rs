//! Registry for managing per-processor state.

use std::iter;
use std::sync::OnceLock;

use many_cpus::{ProcessorId, SystemHardware};

use crate::ProcessorState;

/// Registry for managing per-processor state.
///
/// This registry maintains the state for each processor in the system. It allows
/// lazy initialization of processor states, ensuring that resources are only
/// allocated for processors that actually schedule tasks.
///
/// The registry is indexed by `ProcessorId`, which corresponds to the logical
/// processor ID assigned by the OS.
pub(crate) struct ProcessorRegistry {
    states: Box<[OnceLock<ProcessorState>]>,
}

impl ProcessorRegistry {
    pub(crate) fn new(hardware: &SystemHardware) -> Self {
        let max_processors = hardware.max_processor_count();
        let states = iter::repeat_with(OnceLock::new)
            .take(max_processors)
            .collect::<Vec<_>>()
            .into_boxed_slice();

        Self { states }
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "processor_id is guaranteed to be in bounds by SystemHardware"
    )]
    pub(crate) fn get_or_init(&self, processor_id: ProcessorId) -> &ProcessorState {
        self.states[processor_id as usize].get_or_init(ProcessorState::new)
    }

    #[cfg(test)]
    #[expect(
        clippy::indexing_slicing,
        reason = "processor_id is guaranteed to be in bounds by HardwareInfo"
    )]
    pub(crate) fn get(&self, processor_id: ProcessorId) -> Option<&ProcessorState> {
        self.states[processor_id as usize].get()
    }

    #[cfg_attr(test, mutants::skip)] // Removing this causes timeouts (workers never stop)
    pub(crate) fn signal_shutdown_all(&self) {
        for state in &self.states {
            if let Some(s) = state.get() {
                s.signal_shutdown();
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn initialized_count(&self) -> usize {
        self.states.iter().filter(|s| s.get().is_some()).count()
    }

    #[cfg(test)]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "index is guaranteed to fit in ProcessorId because the registry is sized exactly to max_processor_count"
    )]
    pub(crate) fn iter_initialized(&self) -> impl Iterator<Item = (ProcessorId, &ProcessorState)> {
        self.states
            .iter()
            .enumerate()
            .filter_map(|(id, state)| state.get().map(|s| (id as ProcessorId, s)))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::atomic::Ordering;

    use many_cpus::fake::{HardwareBuilder, ProcessorBuilder};

    use super::*;

    fn fake_hardware(processor_count: usize) -> SystemHardware {
        let builder = (0..processor_count).fold(HardwareBuilder::new(), |b, i| {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "test values are small enough to fit"
            )]
            b.processor(ProcessorBuilder::new().id(i as u32))
        });
        SystemHardware::fake(builder)
    }

    #[test]
    fn new_creates_registry_with_correct_capacity() {
        let hardware = fake_hardware(4);
        let registry = ProcessorRegistry::new(&hardware);

        // Initially no processors should be initialized.
        assert_eq!(registry.initialized_count(), 0);
    }

    #[test]
    fn get_returns_none_for_uninitialized() {
        let hardware = fake_hardware(4);
        let registry = ProcessorRegistry::new(&hardware);

        assert!(registry.get(0).is_none());
        assert!(registry.get(3).is_none());
    }

    #[test]
    fn get_or_init_initializes_state() {
        let hardware = fake_hardware(4);
        let registry = ProcessorRegistry::new(&hardware);

        let state = registry.get_or_init(0);
        assert_eq!(state.get_tasks_spawned(), 0);

        assert_eq!(registry.initialized_count(), 1);
    }

    #[test]
    fn get_or_init_returns_same_state() {
        let hardware = fake_hardware(4);
        let registry = ProcessorRegistry::new(&hardware);

        let state1 = registry.get_or_init(0);
        state1.record_task_spawned();

        let state2 = registry.get_or_init(0);
        assert_eq!(state2.get_tasks_spawned(), 1);

        // Should still only have one initialized processor.
        assert_eq!(registry.initialized_count(), 1);
    }

    #[test]
    fn signal_shutdown_all_signals_all_initialized() {
        let hardware = fake_hardware(4);
        let registry = ProcessorRegistry::new(&hardware);

        // Initialize a few processors.
        let state0 = registry.get_or_init(0);
        let state1 = registry.get_or_init(1);

        assert!(!state0.shutdown_flag.load(Ordering::Acquire));
        assert!(!state1.shutdown_flag.load(Ordering::Acquire));

        registry.signal_shutdown_all();

        assert!(state0.shutdown_flag.load(Ordering::Acquire));
        assert!(state1.shutdown_flag.load(Ordering::Acquire));
    }

    #[test]
    fn iter_initialized_yields_only_initialized() {
        let hardware = fake_hardware(4);
        let registry = ProcessorRegistry::new(&hardware);

        registry.get_or_init(0);
        registry.get_or_init(2);

        let initialized: Vec<_> = registry.iter_initialized().collect();
        assert_eq!(initialized.len(), 2);

        let ids: Vec<_> = initialized.iter().map(|(id, _)| *id).collect();
        assert!(ids.contains(&0));
        assert!(ids.contains(&2));
    }

    #[test]
    fn multiple_processors_can_be_initialized() {
        let hardware = fake_hardware(3);
        let registry = ProcessorRegistry::new(&hardware);

        registry.get_or_init(0);
        registry.get_or_init(1);
        registry.get_or_init(2);

        assert_eq!(registry.initialized_count(), 3);
    }

    #[test]
    fn registry_with_single_processor() {
        let hardware = fake_hardware(1);
        let registry = ProcessorRegistry::new(&hardware);

        let state = registry.get_or_init(0);
        assert_eq!(state.get_tasks_spawned(), 0);
        assert_eq!(registry.initialized_count(), 1);
    }
}
