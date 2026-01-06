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
    pub(crate) fn new() -> Self {
        let max_processors = SystemHardware::current().max_processor_count();
        let states = iter::repeat_with(OnceLock::new)
            .take(max_processors)
            .collect::<Vec<_>>()
            .into_boxed_slice();

        Self { states }
    }

    #[allow(
        clippy::indexing_slicing,
        reason = "processor_id is guaranteed to be in bounds by SystemHardware"
    )]
    pub(crate) fn get_or_init(&self, processor_id: ProcessorId) -> &ProcessorState {
        self.states[processor_id as usize].get_or_init(ProcessorState::new)
    }

    #[cfg(test)]
    #[allow(
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
    #[allow(
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

    use super::*;

    // We cannot easily test ProcessorRegistry in unit tests because it depends on
    // HardwareInfo::max_processor_count() which queries the real hardware.
    // Integration tests will cover this functionality.

    #[cfg_attr(miri, ignore)]
    #[test]
    fn new_creates_registry() {
        let registry = ProcessorRegistry::new();

        // Initially no processors should be initialized.
        assert_eq!(registry.initialized_count(), 0);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn get_returns_none_for_uninitialized() {
        let registry = ProcessorRegistry::new();

        assert!(registry.get(0).is_none());
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn get_or_init_initializes_state() {
        let registry = ProcessorRegistry::new();

        let state = registry.get_or_init(0);
        assert_eq!(state.get_tasks_spawned(), 0);

        assert_eq!(registry.initialized_count(), 1);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn get_or_init_returns_same_state() {
        let registry = ProcessorRegistry::new();

        let state1 = registry.get_or_init(0);
        state1.record_task_spawned();

        let state2 = registry.get_or_init(0);
        assert_eq!(state2.get_tasks_spawned(), 1);

        // Should still only have one initialized processor.
        assert_eq!(registry.initialized_count(), 1);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn signal_shutdown_all_signals_all_initialized() {
        let registry = ProcessorRegistry::new();

        // Initialize a few processors.
        let state0 = registry.get_or_init(0);
        let state1 = registry.get_or_init(1);

        assert!(!state0.shutdown_flag.load(Ordering::Acquire));
        assert!(!state1.shutdown_flag.load(Ordering::Acquire));

        registry.signal_shutdown_all();

        assert!(state0.shutdown_flag.load(Ordering::Acquire));
        assert!(state1.shutdown_flag.load(Ordering::Acquire));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn iter_initialized_yields_only_initialized() {
        let registry = ProcessorRegistry::new();

        registry.get_or_init(0);
        registry.get_or_init(2);

        let initialized: Vec<_> = registry.iter_initialized().collect();
        assert_eq!(initialized.len(), 2);

        let ids: Vec<_> = initialized.iter().map(|(id, _)| *id).collect();
        assert!(ids.contains(&0));
        assert!(ids.contains(&2));
    }
}
