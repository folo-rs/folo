use std::{
    cmp,
    fmt::{Display, Write},
    iter,
    num::NonZero,
};

use foldhash::{HashMap, HashMapExt};

use crate::{EventName, GLOBAL_REGISTRY, Magnitude, ObservationBagSnapshot};

/// A human- and machine-readable report about observed occurrences of events.
///
/// For human-readable output, use the `Display` trait implementation. This is intended
/// for writing to a terminal and uses only the basic ASCII character set.
///
/// For machine-readable output, inspect report contents via the provided methods.
#[derive(Debug)]
pub struct Report {
    // Sorted by event name, ascending.
    events: Box<[EventMetrics]>,
}

impl Report {
    /// Generates a report by collecting all metrics for all events.
    ///
    /// # Panics
    ///
    /// Panics if the same event is registered on different threads with a different configuration.
    #[must_use]
    pub fn collect() -> Self {
        // We must first collect all observations from all threads and merge them per-event.
        let mut event_name_to_merged_snapshot = HashMap::new();

        GLOBAL_REGISTRY.inspect(|observation_bags| {
            for (event_name, observation_bag) in observation_bags {
                let snapshot = observation_bag.snapshot();

                // Merge the snapshot into the existing one for this event name.
                event_name_to_merged_snapshot
                    .entry(event_name.clone())
                    .and_modify(|existing_snapshot: &mut ObservationBagSnapshot| {
                        existing_snapshot.merge_from(&snapshot);
                    })
                    .or_insert(snapshot);
            }
        });

        // Now that we have the data set, we can form the report.
        let mut events = event_name_to_merged_snapshot
            .into_iter()
            .map(|(event_name, snapshot)| EventMetrics::new(event_name, snapshot))
            .collect::<Vec<_>>();

        // Sort the events by name.
        events.sort_by_key(|event_metrics| event_metrics.name().clone());

        Self {
            events: events.into_boxed_slice(),
        }
    }

    /// Iterates through all the events in the report, allowing access to their metrics.
    pub fn events(&self) -> impl Iterator<Item = &EventMetrics> {
        self.events.iter()
    }
}

impl Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for event in &self.events {
            writeln!(f, "{event}")?;
        }

        Ok(())
    }
}

/// A human- and machine-readable report about observed occurrences of a single event.
///
/// Part of a collected [`Report`],
#[derive(Debug)]
pub struct EventMetrics {
    name: EventName,

    count: u64,
    sum: Magnitude,

    // 0 if there are no observations.
    mean: Magnitude,

    // None if the event was not configured to generate a histogram.
    histogram: Option<Histogram>,
}

impl EventMetrics {
    pub(crate) fn new(name: EventName, snapshot: ObservationBagSnapshot) -> Self {
        let count = snapshot.count;
        let sum = snapshot.sum;

        #[expect(
            clippy::arithmetic_side_effects,
            reason = "NonZero protects against division by zero"
        )]
        #[expect(
            clippy::integer_division,
            reason = "we accept that we lose the remainder - 100% precision not required"
        )]
        let mean = Magnitude::try_from(count)
            .ok()
            .and_then(NonZero::new)
            .map_or(0, |count| sum / count.get());

        let histogram = if snapshot.bucket_magnitudes.is_empty() {
            None
        } else {
            // We now need to synthesize the `Magnitude::MAX` bucket for the histogram.
            // This is just "whatever is left after the configured buckets".
            let plus_infinity_bucket_count = snapshot
                .count
                .saturating_sub(snapshot.bucket_counts.iter().sum::<u64>());

            Some(Histogram {
                magnitudes: snapshot.bucket_magnitudes,
                counts: snapshot.bucket_counts,
                plus_infinity_bucket_count,
            })
        };

        Self {
            name,
            count,
            sum,
            mean,
            histogram,
        }
    }

    /// The name of the event associated with these metrics.
    #[must_use]
    pub fn name(&self) -> &EventName {
        &self.name
    }

    /// Total number of occurrences that have been observed.
    #[must_use]
    pub fn count(&self) -> u64 {
        self.count
    }

    /// Sum of the magnitudes of all observed occurrences.
    #[must_use]
    pub fn sum(&self) -> Magnitude {
        self.sum
    }

    /// Mean magnitude of all observed occurrences.
    ///
    /// If there are no observations, this will be zero.
    #[must_use]
    pub fn mean(&self) -> Magnitude {
        self.mean
    }

    /// The histogram of observed magnitudes (if configured).
    ///
    /// `None` if the event [was not configured to generate a histogram][1].
    ///
    /// [1]: crate::EventBuilder::histogram
    #[must_use]
    pub fn histogram(&self) -> Option<&Histogram> {
        self.histogram.as_ref()
    }
}

impl Display for EventMetrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: ", self.name)?;

        if self.count == 0 {
            // If there is no recorded data, we just report a flat zero no questions asked.
            writeln!(f, "0")?;
            return Ok(());
        }

        #[expect(
            clippy::cast_possible_wrap,
            reason = "intentional wrap - crate policy is that values out of safe range may be mangled"
        )]
        let count_as_magnitude = self.count as Magnitude;

        if count_as_magnitude == self.sum && self.histogram.is_none() {
            // If we observe that only magnitude 1 events were recorded and there are no buckets,
            // we treat this event as a bare counter and only emit the count.
            //
            // This is a heuristic: we might be wrong (e.g. 0 + 2 looks like 1 + 1) but given that
            // this is a display for manual reading, we can afford to be wrong in some cases if it
            // makes the typical case more readable.
            writeln!(f, "{} (counter)", self.count)?;
        } else {
            writeln!(f, "{}; sum {}; mean {}", self.count, self.sum, self.mean)?;
        }

        // If there is no histogram to report (because there are no buckets defined), we are done.
        if let Some(histogram) = &self.histogram {
            writeln!(f, "{histogram}")?;
        }

        Ok(())
    }
}

/// A histogram of observed event magnitudes.
///
/// A collected [`Report`] will contain a histogram
/// for each event that was configured to generate one.
#[derive(Debug)]
pub struct Histogram {
    /// Sorted, ascending.
    ///
    /// When iterating buckets, we always append a synthetic `Magnitude::MAX` bucket.
    /// This is never included in the original magnitudes, always synthetic.
    magnitudes: &'static [Magnitude],

    counts: Box<[u64]>,

    /// Occurrences that did not fit into any of the buckets.
    /// We map these to a synthetic bucket with `Magnitude::MAX`.
    plus_infinity_bucket_count: u64,
}

impl Histogram {
    /// Iterates over the magnitudes of the histogram buckets, in ascending order.
    ///
    /// Each bucket counts the number of events that are less than or equal to the corresponding
    /// magnitude. Each occurrence of an event is counted only once, in the first bucket that can
    /// accept it.
    ///
    /// The last bucket always has the magnitude `Magnitude::MAX`, counting
    /// occurrences that do not fit into any of the previous buckets.
    pub fn magnitudes(&self) -> impl Iterator<Item = Magnitude> {
        self.magnitudes
            .iter()
            .copied()
            .chain(iter::once(Magnitude::MAX))
    }

    /// Iterates over the count of occurrences in each bucket,
    /// including the last `Magnitude::MAX` bucket.
    ///
    /// Each bucket counts the number of events that are less than or equal to the corresponding
    /// magnitude. Each occurrence of an event is counted only once, in the first bucket that can
    /// accept it.
    pub fn counts(&self) -> impl Iterator<Item = u64> {
        self.counts
            .iter()
            .copied()
            .chain(iter::once(self.plus_infinity_bucket_count))
    }

    /// Iterates over the histogram buckets as `(magnitude, count)` pairs,
    /// in ascending order of magnitudes.
    ///
    /// Each bucket counts the number of events that are less than or equal to the corresponding
    /// magnitude. Each occurrence of an event is counted only once, in the first bucket that can
    /// accept it.
    ///
    /// The last bucket always has the magnitude `Magnitude::MAX`, counting
    /// occurrences that do not fit into any of the previous buckets.
    pub fn buckets(&self) -> impl Iterator<Item = (Magnitude, u64)> {
        self.magnitudes().zip(self.counts())
    }
}

/// We auto-scale histogram bars when rendering the report. This is the number of characters
/// that we use to represent the maximum bucket value in the histogram.
///
/// Histograms may be smaller than this, as well, because one character will never represent
/// less than one event (so if the max value is 3, the histogram render will be 3 characters wide).
const HISTOGRAM_BAR_WIDTH_CHARS: u64 = 50;

impl Display for Histogram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let buckets = self.buckets().collect::<Vec<_>>();

        // We use the maximum bucket value to auto-scale the histogram bars.
        let max_bucket_value = buckets
            .iter()
            .map(|(_, count)| *count)
            .max()
            .expect("a histogram always has at least one bucket by definition (+inf)");

        // Each character in the histogram bar represents this many events for auto-scaling
        // purposes. We use integers, so this can suffer from aliasing effects if there are
        // not many events. That's fine - the relative sizes will still be fine and the numbers
        // will give the ground truth even if the rendering is not perfect.
        #[expect(
            clippy::integer_division,
            reason = "we accept the loss of precision here - the bar might not always reach 100% of desired width"
        )]
        let count_per_char = cmp::max(max_bucket_value / HISTOGRAM_BAR_WIDTH_CHARS, 1);

        // We measure the dynamic parts of the string to know how much padding to add.

        // We write the observation counts here (both in measurement phase and when rendering).
        let mut count_str = String::new();

        // What is the widest event count string for any bucket? Affects padding.
        let widest_count = buckets.iter().fold(0, |current, bucket| {
            count_str.clear();
            write!(&mut count_str, "{}", bucket.1)
                .expect("we expect writing integer to String to be infallible");
            cmp::max(current, count_str.len())
        });

        // We write the bucket upper bounds here (both in measurement phase and when rendering).
        let mut upper_bound_str = String::new();

        // What is the widest upper bound string for any bucket? Affects padding.
        let widest_upper_bound = buckets.iter().fold(0, |current, bucket| {
            upper_bound_str.clear();

            if bucket.0 == Magnitude::MAX {
                // We use "+inf" for the upper bound of the last bucket.
                write!(&mut upper_bound_str, "+inf")
                    .expect("we expect writing integer to String to be infallible");
            } else {
                write!(&mut upper_bound_str, "{}", bucket.0)
                    .expect("we expect writing integer to String to be infallible");
            }

            cmp::max(current, upper_bound_str.len())
        });

        for (magnitude, count) in buckets {
            upper_bound_str.clear();

            if magnitude == Magnitude::MAX {
                // We use "+inf" for the upper bound of the last bucket.
                write!(&mut upper_bound_str, "+inf")?;
            } else {
                // Otherwise, we write the magnitude as is.
                write!(&mut upper_bound_str, "{magnitude}")?;
            }

            let padding_needed = widest_upper_bound.saturating_sub(upper_bound_str.len());
            for _ in 0..padding_needed {
                upper_bound_str.insert(0, ' ');
            }

            count_str.clear();
            write!(&mut count_str, "{count}")?;

            let padding_needed = widest_count.saturating_sub(count_str.len());
            for _ in 0..padding_needed {
                count_str.insert(0, ' ');
            }

            write!(f, "value <= {upper_bound_str} [ {count_str} ]: ")?;

            #[expect(
                clippy::arithmetic_side_effects,
                reason = "guarded by count_per_char being max'ed to at least 1 above"
            )]
            #[expect(
                clippy::integer_division,
                reason = "intentional loss of precision - this is a vibe render, not intended for fine accuracy"
            )]
            let histogram_bar_width = count / count_per_char;
            for _ in 0..histogram_bar_width {
                write!(f, "∎")?;
            }
            writeln!(f)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use super::*;

    #[test]
    fn histogram_properties_reflect_reality() {
        let magnitudes = &[-5, 1, 10, 100];
        let counts = &[66, 5, 3, 2];

        let histogram = Histogram {
            magnitudes,
            counts: Vec::from(counts).into_boxed_slice(),
            plus_infinity_bucket_count: 1,
        };

        assert_eq!(
            histogram.magnitudes().collect::<Vec<_>>(),
            magnitudes
                .iter()
                .copied()
                .chain(iter::once(Magnitude::MAX))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            histogram.counts().collect::<Vec<_>>(),
            counts
                .iter()
                .copied()
                .chain(iter::once(1))
                .collect::<Vec<_>>()
        );

        let buckets: Vec<_> = histogram.buckets().collect();
        assert_eq!(buckets.len(), 5);
        assert_eq!(buckets[0], (-5, 66));
        assert_eq!(buckets[1], (1, 5));
        assert_eq!(buckets[2], (10, 3));
        assert_eq!(buckets[3], (100, 2));
        assert_eq!(buckets[4], (Magnitude::MAX, 1));
    }

    #[test]
    fn histogram_display_contains_expected_information() {
        let magnitudes = &[-5, 1, 10, 100];
        let counts = &[666666, 5, 3, 2];

        let histogram = Histogram {
            magnitudes,
            counts: Vec::from(counts).into_boxed_slice(),
            plus_infinity_bucket_count: 1,
        };

        let mut output = String::new();
        write!(&mut output, "{histogram}").unwrap();

        println!("{output}");

        // We expect each bucket to be displayed, except the MAX one should say "+inf"
        // instead of the actual value (because the numeric value is too big for good UX).
        // We check for the specific format we expect to see here (change test if we change format).
        assert!(output.contains("value <=   -5 [ 666666 ]: "));
        assert!(output.contains("value <=    1 [      5 ]: "));
        assert!(output.contains("value <=   10 [      3 ]: "));
        assert!(output.contains("value <=  100 [      2 ]: "));
        assert!(output.contains("value <= +inf [      1 ]: "));

        // We do not want to reproduce the auto-scaling logic here, so let's just ensure that
        // the lines are not hilariously long (e.g. 66666 chars), as a basic sanity check.
        // NB! Recall that String::len() counts BYTES and that the "boxes" we draw are non-ASCII
        // characters that take up more than one byte each! So we leave some extra room with a
        // x5 multiplier to give it some leeway - we just want to detect insane line lengths.
        #[expect(clippy::cast_possible_truncation, reason = "safe range, tiny values")]
        let max_acceptable_line_length = (HISTOGRAM_BAR_WIDTH_CHARS * 5) as usize;

        for line in output.lines() {
            assert!(
                line.len() < max_acceptable_line_length,
                "line is too long: {line}"
            );
        }
    }

    #[test]
    fn event_properties_reflect_reality() {
        let event_name = "test_event".to_string();
        let count = 50;
        let sum = Magnitude::from(1000);
        let mean = Magnitude::from(20);

        let histogram = Histogram {
            magnitudes: &[1, 10, 100],
            counts: vec![5, 3, 2].into_boxed_slice(),
            plus_infinity_bucket_count: 1,
        };

        let event_metrics = EventMetrics {
            name: event_name.clone().into(),
            count,
            sum,
            mean,
            histogram: Some(histogram),
        };

        assert_eq!(event_metrics.name(), &event_name);
        assert_eq!(event_metrics.count(), count);
        assert_eq!(event_metrics.sum(), sum);
        assert_eq!(event_metrics.mean(), mean);
        assert!(event_metrics.histogram().is_some());
    }

    #[test]
    fn event_display_contains_expected_information() {
        let event_name = "test_event".to_string();
        let count = 50;
        let sum = Magnitude::from(1000);
        let mean = Magnitude::from(20);

        let histogram = Histogram {
            magnitudes: &[1, 10, 100],
            counts: vec![5, 3, 2].into_boxed_slice(),
            plus_infinity_bucket_count: 1,
        };

        let event_metrics = EventMetrics {
            name: event_name.clone().into(),
            count,
            sum,
            mean,
            histogram: Some(histogram),
        };

        let mut output = String::new();
        write!(&mut output, "{event_metrics}").unwrap();

        println!("{output}");

        // We expect the output to contain the event name and the metrics.
        // We do not prescribe the exact format here, as it is not so critical.
        assert!(output.contains(&event_name));
        assert!(output.contains(&count.to_string()));
        assert!(output.contains(&sum.to_string()));
        assert!(output.contains(&mean.to_string()));
        assert!(output.contains("value <= +inf [ 1 ]: "));
    }

    #[test]
    fn report_display_contains_expected_events() {
        let event1 = EventMetrics {
            name: "event1".to_string().into(),
            count: 10,
            sum: Magnitude::from(100),
            mean: Magnitude::from(10),
            histogram: None,
        };

        let event2 = EventMetrics {
            name: "event2".to_string().into(),
            count: 5,
            sum: Magnitude::from(50),
            mean: Magnitude::from(10),
            histogram: None,
        };

        let report = Report {
            events: vec![event1, event2].into_boxed_slice(),
        };

        let mut output = String::new();
        write!(&mut output, "{report}").unwrap();

        println!("{output}");

        // We expect the output to contain both events.
        assert!(output.contains("event1"));
        assert!(output.contains("event2"));
    }
}
