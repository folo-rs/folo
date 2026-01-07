use std::fmt::{self, Display, Write};
use std::num::NonZero;
use std::{cmp, iter};

use foldhash::{HashMap, HashMapExt};
use new_zealand::nz;

use crate::{EventName, GLOBAL_REGISTRY, Magnitude, ObservationBagSnapshot, Observations};

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
    /// # Example
    ///
    /// ```
    /// use nm::{Event, Report};
    ///
    /// thread_local! {
    ///     static TEST_EVENT: Event = Event::builder()
    ///         .name("test_event")
    ///         .build();
    /// }
    ///
    /// // Observe some events first
    /// TEST_EVENT.with(|e| e.observe_once());
    ///
    /// let report = Report::collect();
    /// println!("{}", report);
    /// ```
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
    ///
    /// # Example
    ///
    /// ```
    /// use nm::{Event, Report};
    ///
    /// thread_local! {
    ///     static TEST_EVENT: Event = Event::builder()
    ///         .name("test_event")
    ///         .build();
    /// }
    ///
    /// // Observe some events first
    /// TEST_EVENT.with(|e| e.observe_once());
    ///
    /// let report = Report::collect();
    ///
    /// for event in report.events() {
    ///     println!("Event: {}, Count: {}", event.name(), event.count());
    /// }
    /// ```
    pub fn events(&self) -> impl Iterator<Item = &EventMetrics> {
        self.events.iter()
    }

    /// Creates a `Report` instance with fake data for testing purposes.
    ///
    /// This constructor is only available with the `test-util` feature and allows
    /// creating arbitrary test data without touching global state.
    #[cfg(any(test, feature = "test-util"))]
    #[must_use]
    pub fn fake(events: Vec<EventMetrics>) -> Self {
        Self {
            events: events.into_boxed_slice(),
        }
    }
}

impl Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
    ///
    /// # Example
    ///
    /// ```
    /// use nm::{Event, Report};
    ///
    /// thread_local! {
    ///     static HTTP_REQUESTS: Event = Event::builder()
    ///         .name("http_requests")
    ///         .build();
    /// }
    ///
    /// HTTP_REQUESTS.with(|e| e.observe_once());
    /// let report = Report::collect();
    ///
    /// for event in report.events() {
    ///     println!("Event name: {}", event.name());
    /// }
    /// ```
    #[must_use]
    pub fn name(&self) -> &EventName {
        &self.name
    }

    /// Total number of occurrences that have been observed.
    ///
    /// # Example
    ///
    /// ```
    /// use nm::{Event, Report};
    ///
    /// thread_local! {
    ///     static HTTP_REQUESTS: Event = Event::builder()
    ///         .name("http_requests")
    ///         .build();
    /// }
    ///
    /// HTTP_REQUESTS.with(|e| e.observe_once());
    /// HTTP_REQUESTS.with(|e| e.observe_once());
    /// let report = Report::collect();
    ///
    /// for event in report.events() {
    ///     println!("Total count: {}", event.count());
    /// }
    /// ```
    #[must_use]
    pub fn count(&self) -> u64 {
        self.count
    }

    /// Sum of the magnitudes of all observed occurrences.
    ///
    /// # Example
    ///
    /// ```
    /// use nm::{Event, Report};
    ///
    /// thread_local! {
    ///     static SENT_BYTES: Event = Event::builder()
    ///         .name("sent_bytes")
    ///         .build();
    /// }
    ///
    /// SENT_BYTES.with(|e| e.observe(1024));
    /// SENT_BYTES.with(|e| e.observe(2048));
    /// let report = Report::collect();
    ///
    /// for event in report.events() {
    ///     println!("Total bytes: {}", event.sum());
    /// }
    /// ```
    #[must_use]
    pub fn sum(&self) -> Magnitude {
        self.sum
    }

    /// Mean magnitude of all observed occurrences.
    ///
    /// If there are no observations, this will be zero.
    ///
    /// # Example
    ///
    /// ```
    /// use nm::{Event, Report};
    ///
    /// thread_local! {
    ///     static RESPONSE_TIME: Event = Event::builder()
    ///         .name("response_time_ms")
    ///         .build();
    /// }
    ///
    /// RESPONSE_TIME.with(|e| e.observe(100));
    /// RESPONSE_TIME.with(|e| e.observe(200));
    /// let report = Report::collect();
    ///
    /// for event in report.events() {
    ///     println!("Average response time: {}ms", event.mean());
    /// }
    /// ```
    #[must_use]
    pub fn mean(&self) -> Magnitude {
        self.mean
    }

    /// The histogram of observed magnitudes (if configured).
    ///
    /// `None` if the event [was not configured to generate a histogram][1].
    ///
    /// # Example
    ///
    /// ```
    /// use nm::{Event, Magnitude, Report};
    ///
    /// const RESPONSE_TIME_BUCKETS_MS: &[Magnitude] = &[10, 50, 100, 500];
    ///
    /// thread_local! {
    ///     static HTTP_RESPONSE_TIME_MS: Event = Event::builder()
    ///         .name("http_response_time_ms")
    ///         .histogram(RESPONSE_TIME_BUCKETS_MS)
    ///         .build();
    /// }
    ///
    /// HTTP_RESPONSE_TIME_MS.with(|e| e.observe(75));
    /// let report = Report::collect();
    ///
    /// for event in report.events() {
    ///     if let Some(histogram) = event.histogram() {
    ///         println!("Histogram for {}", event.name());
    ///         for (bucket_upper_bound, count) in histogram.buckets() {
    ///             println!("  ≤{}: {}", bucket_upper_bound, count);
    ///         }
    ///     }
    /// }
    /// ```
    ///
    /// [1]: crate::EventBuilder::histogram
    #[must_use]
    pub fn histogram(&self) -> Option<&Histogram> {
        self.histogram.as_ref()
    }

    /// Creates an `EventMetrics` instance with fake data for testing purposes.
    ///
    /// This constructor is only available with the `test-util` feature and allows
    /// creating arbitrary test data without touching global state.
    #[cfg(any(test, feature = "test-util"))]
    #[must_use]
    pub fn fake(
        name: impl Into<EventName>,
        count: u64,
        sum: Magnitude,
        histogram: Option<Histogram>,
    ) -> Self {
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

        Self {
            name: name.into(),
            count,
            sum,
            mean,
            histogram,
        }
    }
}

impl Display for EventMetrics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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

    /// Creates a `Histogram` instance with fake data for testing purposes.
    ///
    /// This constructor is only available with the `test-util` feature and allows
    /// creating arbitrary test data without touching global state.
    ///
    /// The `magnitudes` slice must be sorted in ascending order. The `counts` slice
    /// must have the same length as `magnitudes`. The `plus_infinity_count` is the
    /// count for the synthetic `Magnitude::MAX` bucket.
    #[cfg(any(test, feature = "test-util"))]
    #[must_use]
    pub fn fake(
        magnitudes: &'static [Magnitude],
        counts: Vec<u64>,
        plus_infinity_count: u64,
    ) -> Self {
        Self {
            magnitudes,
            counts: counts.into_boxed_slice(),
            plus_infinity_bucket_count: plus_infinity_count,
        }
    }
}

/// We auto-scale histogram bars when rendering the report. This is the number of characters
/// that we use to represent the maximum bucket value in the histogram.
///
/// Histograms may be smaller than this, as well, because one character will never represent
/// less than one event (so if the max value is 3, the histogram render will be 3 characters wide).
///
/// Due to aliasing effects (have to assign at least 1 item per character), the width may even
/// be greater than this for histograms with very small bucket values. We are not after perfect
/// rendering here, just a close enough approximation that is easy to read.
const HISTOGRAM_BAR_WIDTH_CHARS: u64 = 50;

/// Pre-allocated string of histogram bar characters to avoid allocation during rendering.
/// We make this longer than the typical bar width to handle cases where aliasing causes
/// the bar to exceed the target width.
const HISTOGRAM_BAR_CHARS: &str =
    "∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎∎";

const HISTOGRAM_BAR_CHARS_LEN_BYTES: NonZero<usize> =
    NonZero::new(HISTOGRAM_BAR_CHARS.len()).unwrap();

/// Number of bytes per histogram bar character.
/// The '∎' character is U+25A0 which encodes to 3 bytes in UTF-8.
const BYTES_PER_HISTOGRAM_BAR_CHAR: NonZero<usize> = nz!(3);

impl Display for Histogram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let buckets = self.buckets().collect::<Vec<_>>();

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

        let histogram_scale = HistogramScale::new(self);

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
            histogram_scale.write_bar(count, f)?;

            writeln!(f)?;
        }

        Ok(())
    }
}

/// Represent the auto-scaling logic of the histogram bars, identifying the step size for rendering.
#[derive(Debug)]
struct HistogramScale {
    /// The number of events that each character in the histogram bar represents.
    /// One character is rendered for each `count_per_char` events (rounded down).
    count_per_char: NonZero<u64>,
}

impl HistogramScale {
    fn new(snapshot: &Histogram) -> Self {
        let max_count = snapshot
            .counts()
            .max()
            .expect("a histogram always has at least one bucket by definition (+inf)");

        // Each character in the histogram bar represents this many events for auto-scaling
        // purposes. We use integers, so this can suffer from aliasing effects if there are
        // not many events. That's fine - the relative sizes will still be fine and the numbers
        // will give the ground truth even if the rendering is not perfect.
        #[expect(
            clippy::integer_division,
            reason = "we accept the loss of precision here - the bar might not always reach 100% of desired width or even overshoot it"
        )]
        let count_per_char = NonZero::new(cmp::max(max_count / HISTOGRAM_BAR_WIDTH_CHARS, 1))
            .expect("guarded by max()");

        Self { count_per_char }
    }

    fn write_bar(&self, count: u64, f: &mut impl Write) -> fmt::Result {
        let histogram_bar_width = count
            .checked_div(self.count_per_char.get())
            .expect("division by zero impossible - divisor is NonZero");

        // Note: due to aliasing we can occasionally exceed HISTOGRAM_BAR_WIDTH_CHARS.
        // This is fine - we are not looking for perfect rendering, just close enough.

        let bar_width = usize::try_from(histogram_bar_width).expect("safe range");

        let chars_in_constant = HISTOGRAM_BAR_CHARS_LEN_BYTES
            .get()
            .checked_div(BYTES_PER_HISTOGRAM_BAR_CHAR.get())
            .expect("NonZero - cannot be zero");

        let mut remaining = bar_width;

        while remaining > 0 {
            let chunk_size =
                NonZero::new(remaining.min(chars_in_constant)).expect("guarded by loop condition");

            // Calculate byte length directly: each ∎ character is BYTES_PER_HISTOGRAM_BAR_CHAR bytes in UTF-8.
            let byte_end = chunk_size
                .checked_mul(BYTES_PER_HISTOGRAM_BAR_CHAR)
                .expect("we are seeking into a small constant value, overflow impossible");

            #[expect(
                clippy::string_slice,
                reason = "safe slicing - ∎ characters have known UTF-8 encoding"
            )]
            f.write_str(&HISTOGRAM_BAR_CHARS[..byte_end.get()])?;

            remaining = remaining
                .checked_sub(chunk_size.get())
                .expect("guarded by min() above");
        }

        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
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
    fn report_properties_reflect_reality() {
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

        // This is very boring because the Report type is very boring.
        let events = report.events().collect::<Vec<_>>();

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].name(), "event1");
        assert_eq!(events[1].name(), "event2");
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

    #[test]
    fn event_displayed_as_counter_if_unit_values_and_no_histogram() {
        // The Display output should be heuristically detected as a "counter"
        // and undergo simplified printing if the sum equals the count and if
        // there is no histogram to report.

        let counter = EventMetrics {
            name: "test_event".to_string().into(),
            count: 100,
            sum: Magnitude::from(100),
            mean: Magnitude::from(1),
            histogram: None,
        };

        // sum != count - cannot be a counter.
        let not_counter = EventMetrics {
            name: "test_event".to_string().into(),
            count: 100,
            sum: Magnitude::from(200),
            mean: Magnitude::from(2),
            histogram: None,
        };

        // Has a histogram - cannot be a counter.
        let also_not_counter = EventMetrics {
            name: "test_event".to_string().into(),
            count: 100,
            sum: Magnitude::from(100),
            mean: Magnitude::from(1),
            histogram: Some(Histogram {
                magnitudes: &[],
                counts: Box::new([]),
                plus_infinity_bucket_count: 100,
            }),
        };

        // Neither condition is a match.
        let still_not_counter = EventMetrics {
            name: "test_event".to_string().into(),
            count: 100,
            sum: Magnitude::from(200),
            mean: Magnitude::from(2),
            histogram: Some(Histogram {
                magnitudes: &[],
                counts: Box::new([]),
                plus_infinity_bucket_count: 200,
            }),
        };

        let mut output = String::new();

        write!(&mut output, "{counter}").unwrap();
        assert!(output.contains("100 (counter)"));
        output.clear();

        write!(&mut output, "{not_counter}").unwrap();
        assert!(output.contains("100; sum 200; mean 2"));
        output.clear();

        write!(&mut output, "{also_not_counter}").unwrap();
        assert!(output.contains("100; sum 100; mean 1"));
        output.clear();

        write!(&mut output, "{still_not_counter}").unwrap();
        assert!(output.contains("100; sum 200; mean 2"));
    }

    #[test]
    fn histogram_scale_zero() {
        // Everything is zero, so we render zero bar segments.
        let histogram = Histogram {
            magnitudes: &[1, 2, 3],
            counts: Box::new([0, 0, 0]),
            plus_infinity_bucket_count: 0,
        };

        let histogram_scale = HistogramScale::new(&histogram);

        let mut output = String::new();
        histogram_scale.write_bar(0, &mut output).unwrap();

        assert_eq!(output, "");
    }

    #[test]
    fn histogram_scale_small() {
        // All the buckets have small values, so we do not reach 100% bar width.
        let histogram = Histogram {
            magnitudes: &[1, 2, 3],
            counts: Box::new([1, 2, 3]),
            plus_infinity_bucket_count: 0,
        };

        let histogram_scale = HistogramScale::new(&histogram);

        let mut output = String::new();

        histogram_scale.write_bar(0, &mut output).unwrap();
        assert_eq!(output, "");
        output.clear();

        histogram_scale.write_bar(1, &mut output).unwrap();
        assert_eq!(output, "∎");
        output.clear();

        histogram_scale.write_bar(2, &mut output).unwrap();
        assert_eq!(output, "∎∎");
        output.clear();

        histogram_scale.write_bar(3, &mut output).unwrap();
        assert_eq!(output, "∎∎∎");
    }

    #[test]
    fn histogram_scale_just_over() {
        // All the buckets values just a tiny bit over the desired width.
        let histogram = Histogram {
            magnitudes: &[1, 2, 3],
            counts: Box::new([
                HISTOGRAM_BAR_WIDTH_CHARS + 1,
                HISTOGRAM_BAR_WIDTH_CHARS + 1,
                HISTOGRAM_BAR_WIDTH_CHARS + 1,
            ]),
            plus_infinity_bucket_count: 0,
        };

        let histogram_scale = HistogramScale::new(&histogram);

        let mut output = String::new();

        histogram_scale
            .write_bar(HISTOGRAM_BAR_WIDTH_CHARS + 1, &mut output)
            .unwrap();
        // We expect ∎ repeated HISTOGRAM_BAR_WIDTH_CHARS + 1 times.
        assert_eq!(
            output,
            "∎".repeat(
                usize::try_from(HISTOGRAM_BAR_WIDTH_CHARS + 1).expect("safe range, tiny value")
            )
        );
    }

    #[test]
    fn histogram_scale_large_exact() {
        // The scale is large enough that we render long segments.
        // The numbers divide just right so the bar reaches the desired width.
        let histogram = Histogram {
            magnitudes: &[1, 2, 3],
            counts: Box::new([
                79,
                HISTOGRAM_BAR_WIDTH_CHARS * 100,
                HISTOGRAM_BAR_WIDTH_CHARS * 1000,
            ]),
            plus_infinity_bucket_count: 0,
        };

        let histogram_scale = HistogramScale::new(&histogram);

        let mut output = String::new();

        histogram_scale.write_bar(0, &mut output).unwrap();
        assert_eq!(output, "");
        output.clear();

        histogram_scale
            .write_bar(histogram_scale.count_per_char.get(), &mut output)
            .unwrap();
        assert_eq!(output, "∎");
        output.clear();

        histogram_scale
            .write_bar(HISTOGRAM_BAR_WIDTH_CHARS * 1000, &mut output)
            .unwrap();
        assert_eq!(
            output,
            "∎".repeat(usize::try_from(HISTOGRAM_BAR_WIDTH_CHARS).expect("safe range, tiny value"))
        );
    }

    #[test]
    fn histogram_scale_large_inexact() {
        // The scale is large enough that we render long segments.
        // The numbers divide with a remainder, so we do not reach 100% width.
        let histogram = Histogram {
            magnitudes: &[1, 2, 3],
            counts: Box::new([
                79,
                HISTOGRAM_BAR_WIDTH_CHARS * 100,
                HISTOGRAM_BAR_WIDTH_CHARS * 1000,
            ]),
            plus_infinity_bucket_count: 0,
        };

        let histogram_scale = HistogramScale::new(&histogram);

        let mut output = String::new();
        histogram_scale.write_bar(3, &mut output).unwrap();

        let mut output = String::new();

        histogram_scale.write_bar(0, &mut output).unwrap();
        assert_eq!(output, "");
        output.clear();

        histogram_scale
            .write_bar(histogram_scale.count_per_char.get() - 1, &mut output)
            .unwrap();
        assert_eq!(output, "");
        output.clear();

        histogram_scale
            .write_bar(histogram_scale.count_per_char.get(), &mut output)
            .unwrap();
        assert_eq!(output, "∎");
        output.clear();

        histogram_scale
            .write_bar(HISTOGRAM_BAR_WIDTH_CHARS * 1000 - 1, &mut output)
            .unwrap();
        assert_eq!(
            output,
            "∎".repeat(
                usize::try_from(HISTOGRAM_BAR_WIDTH_CHARS).expect("safe range, tiny value") - 1
            )
        );
    }

    #[test]
    fn histogram_char_byte_count_is_correct() {
        // Verify our assumption that ∎ is BYTES_PER_HISTOGRAM_BAR_CHAR bytes in UTF-8.
        assert_eq!("∎".len(), BYTES_PER_HISTOGRAM_BAR_CHAR.get());

        // Verify that our constant string has the expected byte length.
        let expected_chars = HISTOGRAM_BAR_CHARS.chars().count();
        let expected_bytes = expected_chars * BYTES_PER_HISTOGRAM_BAR_CHAR.get();
        assert_eq!(HISTOGRAM_BAR_CHARS.len(), expected_bytes);
    }

    #[test]
    fn event_metrics_display_zero_count_reports_flat_zero() {
        // This tests the "If there is no recorded data, we just report a flat zero
        // no questions asked." branch in the Display impl.
        let snapshot = ObservationBagSnapshot {
            count: 0,
            sum: 0,
            bucket_magnitudes: &[],
            bucket_counts: Box::new([]),
        };

        let metrics = EventMetrics::new("zero_event".into(), snapshot);

        let output = format!("{metrics}");

        // The output should contain the event name followed by ": 0".
        assert!(output.contains("zero_event: 0"));

        // It should be a short output (just the name and zero, plus newline).
        assert_eq!(output.trim(), "zero_event: 0");
    }

    #[test]
    fn event_metrics_new_zero_count_empty_buckets() {
        let snapshot = ObservationBagSnapshot {
            count: 0,
            sum: 0,
            bucket_magnitudes: &[],
            bucket_counts: Box::new([]),
        };

        let metrics = EventMetrics::new("empty_event".into(), snapshot);

        assert_eq!(metrics.name(), "empty_event");
        assert_eq!(metrics.count(), 0);
        assert_eq!(metrics.sum(), 0);
        assert_eq!(metrics.mean(), 0);
        assert!(metrics.histogram().is_none());
    }

    #[test]
    fn event_metrics_new_non_zero_count_empty_buckets() {
        let snapshot = ObservationBagSnapshot {
            count: 10,
            sum: 100,
            bucket_magnitudes: &[],
            bucket_counts: Box::new([]),
        };

        let metrics = EventMetrics::new("sum_event".into(), snapshot);

        assert_eq!(metrics.name(), "sum_event");
        assert_eq!(metrics.count(), 10);
        assert_eq!(metrics.sum(), 100);
        assert_eq!(metrics.mean(), 10); // 100 / 10 = 10
        assert!(metrics.histogram().is_none());
    }

    #[test]
    fn event_metrics_new_non_zero_count_with_buckets() {
        // Create a snapshot with bucket data.
        // Buckets: [-10, 0, 10, 100]
        // Counts in buckets: [2, 3, 4, 5] = 14 total in buckets
        // Total count: 20 (so 6 are in +inf bucket)
        let snapshot = ObservationBagSnapshot {
            count: 20,
            sum: 500,
            bucket_magnitudes: &[-10, 0, 10, 100],
            bucket_counts: vec![2, 3, 4, 5].into_boxed_slice(),
        };

        let metrics = EventMetrics::new("histogram_event".into(), snapshot);

        assert_eq!(metrics.name(), "histogram_event");
        assert_eq!(metrics.count(), 20);
        assert_eq!(metrics.sum(), 500);
        assert_eq!(metrics.mean(), 25); // 500 / 20 = 25

        let histogram = metrics.histogram().expect("histogram should be present");

        // Verify bucket magnitudes include the synthetic +inf bucket.
        let magnitudes: Vec<_> = histogram.magnitudes().collect();
        assert_eq!(magnitudes, vec![-10, 0, 10, 100, Magnitude::MAX]);

        // Verify bucket counts include the plus_infinity_bucket_count.
        // plus_infinity = 20 - (2+3+4+5) = 20 - 14 = 6
        let counts: Vec<_> = histogram.counts().collect();
        assert_eq!(counts, vec![2, 3, 4, 5, 6]);

        // Verify buckets() returns correct pairs.
        let buckets: Vec<_> = histogram.buckets().collect();
        assert_eq!(buckets.len(), 5);
        assert_eq!(buckets[0], (-10, 2));
        assert_eq!(buckets[1], (0, 3));
        assert_eq!(buckets[2], (10, 4));
        assert_eq!(buckets[3], (100, 5));
        assert_eq!(buckets[4], (Magnitude::MAX, 6));
    }
}
