//! State tracking for delta computation between collections.

use foldhash::HashMap;
use nm::{EventName, Magnitude};

/// Tracks the previous state of nm metrics for delta computation.
///
/// Counter-type metrics (count, histogram buckets) are cumulative in nm but require
/// delta computation for OpenTelemetry. Gauge-type metrics (sum) are set directly.
#[derive(Debug, Default)]
pub(crate) struct CollectionState {
    /// Previous state per event name.
    events: HashMap<EventName, EventState>,
}

impl CollectionState {
    /// Creates a new empty collection state.
    pub(crate) fn new() -> Self {
        Self {
            events: HashMap::default(),
        }
    }

    /// Gets or creates the state for an event.
    pub(crate) fn event_state(&mut self, name: &EventName) -> &mut EventState {
        self.events.entry(name.clone()).or_default()
    }
}

/// Previous state for a single event.
#[derive(Debug, Default)]
pub(crate) struct EventState {
    /// Previous cumulative count.
    pub(crate) count: u64,

    /// Previous cumulative histogram bucket counts (already converted to cumulative format).
    /// Indexed by bucket index.
    pub(crate) histogram_buckets: Vec<u64>,
}

impl EventState {
    /// Computes the delta for the count metric.
    ///
    /// Returns the delta and updates internal state.
    pub(crate) fn count_delta(&mut self, current: u64) -> u64 {
        let delta = current.saturating_sub(self.count);
        self.count = current;
        delta
    }

    /// Computes deltas for histogram bucket counts.
    ///
    /// Takes nm's non-cumulative bucket counts, converts to cumulative format,
    /// computes deltas from previous state, and updates internal state.
    ///
    /// Returns a vector of `(magnitude, cumulative_count, delta)` for each bucket.
    pub(crate) fn histogram_deltas(
        &mut self,
        magnitudes: impl Iterator<Item = Magnitude>,
        non_cumulative_counts: impl Iterator<Item = u64>,
    ) -> Vec<(Magnitude, u64, u64)> {
        // Convert non-cumulative to cumulative counts.
        let cumulative_counts = to_cumulative(non_cumulative_counts);

        // Ensure we have enough space in our previous state vector.
        if self.histogram_buckets.len() < cumulative_counts.len() {
            self.histogram_buckets.resize(cumulative_counts.len(), 0);
        }

        // Compute deltas.
        let mut result = Vec::with_capacity(cumulative_counts.len());

        for (i, (magnitude, cumulative)) in magnitudes
            .zip(cumulative_counts.iter().copied())
            .enumerate()
        {
            let previous = self.histogram_buckets.get(i).copied().unwrap_or_default();
            let delta = cumulative.saturating_sub(previous);

            // Update previous state.
            if let Some(slot) = self.histogram_buckets.get_mut(i) {
                *slot = cumulative;
            }

            result.push((magnitude, cumulative, delta));
        }

        result
    }
}

/// Converts non-cumulative bucket counts to cumulative format.
///
/// nm stores per-bucket counts: `[5, 12, 8]` means 5 in bucket 0, 12 in bucket 1, etc.
/// Prometheus expects cumulative: `[5, 17, 25]` means 5 ≤ bound0, 17 ≤ bound1, etc.
fn to_cumulative(non_cumulative: impl Iterator<Item = u64>) -> Vec<u64> {
    let mut cumulative = Vec::new();
    let mut running_total = 0_u64;

    for count in non_cumulative {
        running_total = running_total.saturating_add(count);
        cumulative.push(running_total);
    }

    cumulative
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn to_cumulative_empty() {
        let result = to_cumulative(std::iter::empty());
        assert!(result.is_empty());
    }

    #[test]
    fn to_cumulative_single() {
        let result = to_cumulative(std::iter::once(5));
        assert_eq!(result, vec![5]);
    }

    #[test]
    fn to_cumulative_multiple() {
        let result = to_cumulative([5, 12, 8, 3, 2].into_iter());
        assert_eq!(result, vec![5, 17, 25, 28, 30]);
    }

    #[test]
    fn event_state_count_delta_first_collection() {
        let mut state = EventState::default();
        let delta = state.count_delta(100);
        assert_eq!(delta, 100);
        assert_eq!(state.count, 100);
    }

    #[test]
    fn event_state_count_delta_subsequent_collections() {
        let mut state = EventState::default();

        let delta1 = state.count_delta(100);
        assert_eq!(delta1, 100);

        let delta2 = state.count_delta(150);
        assert_eq!(delta2, 50);

        let delta3 = state.count_delta(150);
        assert_eq!(delta3, 0);

        let delta4 = state.count_delta(200);
        assert_eq!(delta4, 50);
    }

    #[test]
    fn event_state_histogram_deltas_first_collection() {
        let mut state = EventState::default();

        let magnitudes = [10, 50, 100, Magnitude::MAX];
        let non_cumulative = [5, 12, 8, 2];

        let result = state.histogram_deltas(magnitudes.into_iter(), non_cumulative.into_iter());

        // First collection: deltas equal cumulative values.
        assert_eq!(result.len(), 4);
        assert_eq!(
            result,
            vec![
                (10, 5, 5),
                (50, 17, 17),
                (100, 25, 25),
                (Magnitude::MAX, 27, 27),
            ]
        );
    }

    #[test]
    fn event_state_histogram_deltas_subsequent_collections() {
        let mut state = EventState::default();

        let magnitudes = [10, 50, 100, Magnitude::MAX];

        // First collection.
        let non_cumulative1 = [5, 12, 8, 2];
        drop(state.histogram_deltas(magnitudes.into_iter(), non_cumulative1.into_iter()));

        // Second collection with more observations.
        let non_cumulative2 = [7, 15, 10, 3];
        let result = state.histogram_deltas(magnitudes.into_iter(), non_cumulative2.into_iter());

        // Cumulative: [7, 22, 32, 35].
        // Previous:   [5, 17, 25, 27].
        // Deltas:     [2, 5, 7, 8].
        assert_eq!(result.len(), 4);
        assert_eq!(
            result,
            vec![
                (10, 7, 2),
                (50, 22, 5),
                (100, 32, 7),
                (Magnitude::MAX, 35, 8),
            ]
        );
    }

    #[test]
    fn collection_state_creates_event_state_on_demand() {
        let mut state = CollectionState::new();

        let event_state = state.event_state(&"test_event".into());
        assert_eq!(event_state.count, 0);

        event_state.count = 100;

        let event_state_again = state.event_state(&"test_event".into());
        assert_eq!(event_state_again.count, 100);
    }

    #[test]
    fn collection_state_tracks_multiple_events() {
        let mut state = CollectionState::new();

        state.event_state(&"event_a".into()).count = 10;
        state.event_state(&"event_b".into()).count = 20;

        assert_eq!(state.event_state(&"event_a".into()).count, 10);
        assert_eq!(state.event_state(&"event_b".into()).count, 20);
    }
}
