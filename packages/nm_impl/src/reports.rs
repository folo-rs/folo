use std::fmt::{self, Display, Write};
use std::num::NonZero;
use std::{cmp, iter};

use crate::{
    EventName, GLOBAL_REGISTRY, GlobalEventRegistry, HashMap, Magnitude, ObservationBagSnapshot,
};

/// A human- and machine-readable report about observed occurrences of events.
///
/// For human-readable output, use the `Display` trait implementation. Its histogram
/// rendering uses Unicode symbols and is intended for Unicode-capable terminals.
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
    /// TEST_EVENT.with(Event::observe_once);
    ///
    /// let report = Report::collect();
    /// println!("{report}");
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the same event is registered on different threads with a different configuration.
    #[must_use]
    pub fn collect() -> Self {
        Self::collect_from(&GLOBAL_REGISTRY)
    }

    fn collect_from(registry: &GlobalEventRegistry) -> Self {
        let mut event_name_to_merged_snapshot: HashMap<EventName, ObservationBagSnapshot> =
            HashMap::default();

        registry.inspect(|observation_bags| {
            for (event_name, observation_bag) in observation_bags {
                if let Some(existing_snapshot) =
                    event_name_to_merged_snapshot.get_mut(event_name.as_ref())
                {
                    existing_snapshot.merge_from_observations(observation_bag);
                } else {
                    event_name_to_merged_snapshot
                        .insert(event_name.clone(), observation_bag.snapshot());
                }
            }
        });

        let events = event_name_to_merged_snapshot
            .into_iter()
            .map(|(event_name, snapshot)| EventMetrics::new(event_name, snapshot))
            .collect::<Vec<_>>();

        Self::from_unsorted_events(events)
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
    /// TEST_EVENT.with(Event::observe_once);
    ///
    /// let report = Report::collect();
    ///
    /// for event in report.events() {
    ///     println!("Event: {}, Count: {}", event.name(), event.count());
    /// }
    /// ```
    #[inline]
    pub fn events(&self) -> impl Iterator<Item = &EventMetrics> {
        self.events.iter()
    }

    /// Constructs a report from preassembled metrics.
    ///
    /// This does not touch the global event registry. It is intended for in-workspace
    /// tests and benchmarks that need to drive code paths expecting a [`Report`].
    #[cfg(any(test, feature = "private-test-util"))]
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[doc(hidden)]
    #[must_use]
    pub fn fake(events: Vec<EventMetrics>) -> Self {
        Self::from_unsorted_events(events)
    }

    fn from_unsorted_events(mut events: Vec<EventMetrics>) -> Self {
        events.sort_by(|left, right| left.name().as_ref().cmp(right.name().as_ref()));

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
/// Part of a collected [`Report`].
#[derive(Debug)]
pub struct EventMetrics {
    name: EventName,

    count: u64,
    sum: Magnitude,

    // None if the event was not configured to generate a histogram.
    histogram: Option<Histogram>,
}

impl EventMetrics {
    pub(crate) fn new(name: EventName, snapshot: ObservationBagSnapshot) -> Self {
        let count = snapshot.count;
        let sum = snapshot.sum;

        let histogram = if snapshot.bucket_magnitudes.is_empty() {
            None
        } else {
            let explicit_bucket_count = snapshot
                .bucket_counts
                .iter()
                .copied()
                .fold(0_u64, u64::wrapping_add);

            // Snapshot fields are loaded independently, so a concurrent observation can make
            // the explicit bucket total appear newer than the overall count. Clamp that
            // logically torn state instead of reporting an impossible negative overflow count.
            let overflow_bucket_count = snapshot.count.saturating_sub(explicit_bucket_count);

            Some(Histogram {
                magnitudes: snapshot.bucket_magnitudes,
                counts: snapshot.bucket_counts,
                overflow_bucket_count,
            })
        };

        Self {
            name,
            count,
            sum,
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
    /// HTTP_REQUESTS.with(Event::observe_once);
    /// let report = Report::collect();
    ///
    /// for event in report.events() {
    ///     println!("Event name: {}", event.name());
    /// }
    /// ```
    #[inline]
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
    /// HTTP_REQUESTS.with(Event::observe_once);
    /// HTTP_REQUESTS.with(Event::observe_once);
    /// let report = Report::collect();
    ///
    /// for event in report.events() {
    ///     println!("Total count: {}", event.count());
    /// }
    /// ```
    #[inline]
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
    #[inline]
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
    #[inline]
    #[must_use]
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "NonZero protects against division by zero"
    )]
    #[expect(
        clippy::integer_division,
        reason = "The reported integral mean is truncated toward zero."
    )]
    pub fn mean(&self) -> Magnitude {
        Magnitude::try_from(self.count)
            .ok()
            .and_then(NonZero::new)
            .map_or(0, |count| self.sum / count.get())
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
    ///             println!("  ≤{bucket_upper_bound}: {count}");
    ///         }
    ///     }
    /// }
    /// ```
    ///
    /// [1]: crate::EventBuilder::histogram
    #[inline]
    #[must_use]
    pub fn histogram(&self) -> Option<&Histogram> {
        self.histogram.as_ref()
    }

    /// Constructs event metrics from precomputed values.
    ///
    /// This does not touch the global event registry. The mean is calculated as
    /// `sum / count`, truncated toward zero; when `count` is zero, the mean is zero.
    /// This function is intended for in-workspace tests and benchmarks.
    #[cfg(any(test, feature = "private-test-util"))]
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[doc(hidden)]
    #[must_use]
    pub fn fake(
        name: impl Into<EventName>,
        count: u64,
        sum: Magnitude,
        histogram: Option<Histogram>,
    ) -> Self {
        Self {
            name: name.into(),
            count,
            sum,
            histogram,
        }
    }
}

impl Display for EventMetrics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.name)?;

        if self.count == 0 {
            writeln!(f, "0")?;
            return Ok(());
        }

        #[expect(
            clippy::cast_possible_wrap,
            reason = "The display contract permits wrapping out-of-range counts."
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
            let mean = self.mean();
            writeln!(f, "{}; sum {}; mean {mean}", self.count, self.sum)?;
        }

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

    /// Occurrences that did not fit into any explicit bucket.
    /// We map these to a synthetic bucket with `Magnitude::MAX`.
    overflow_bucket_count: u64,
}

impl Histogram {
    /// Iterates over the magnitudes of the histogram buckets, in ascending order.
    ///
    /// Each bucket counts observations whose magnitude is less than or equal to its upper bound.
    /// Each observation is counted only once, in the first bucket that accepts it.
    ///
    /// The last bucket always has the magnitude `Magnitude::MAX`, counting
    /// occurrences that do not fit into any of the previous buckets.
    #[inline]
    pub fn magnitudes(&self) -> impl Iterator<Item = Magnitude> {
        self.magnitudes
            .iter()
            .copied()
            .chain(iter::once(Magnitude::MAX))
    }

    /// Iterates over occurrence counts, including the last `Magnitude::MAX` bucket.
    ///
    /// Each bucket counts observations whose magnitude is less than or equal to its upper bound.
    /// Each observation is counted only once, in the first bucket that accepts it.
    #[inline]
    pub fn counts(&self) -> impl Iterator<Item = u64> {
        self.counts
            .iter()
            .copied()
            .chain(iter::once(self.overflow_bucket_count))
    }

    /// Iterates over the histogram buckets as `(magnitude, count)` pairs,
    /// in ascending order of magnitudes.
    ///
    /// Each bucket counts observations whose magnitude is less than or equal to its upper bound.
    /// Each observation is counted only once, in the first bucket that accepts it.
    ///
    /// The last bucket always has the magnitude `Magnitude::MAX`, counting
    /// occurrences that do not fit into any of the previous buckets.
    #[inline]
    pub fn buckets(&self) -> impl Iterator<Item = (Magnitude, u64)> {
        self.magnitudes().zip(self.counts())
    }

    /// Constructs a histogram from raw parts.
    ///
    /// `bucket_upper_bounds` must be sorted in strictly ascending order and must not
    /// contain `Magnitude::MAX`, which is synthesized as the terminal bucket.
    /// `bucket_counts` must have the same length as `bucket_upper_bounds`.
    /// `overflow_bucket_count` is the count for the synthetic terminal bucket.
    ///
    /// This does not touch the global event registry. It is intended for in-workspace
    /// tests and benchmarks.
    ///
    /// # Panics
    ///
    /// Panics if any of the above preconditions are violated.
    #[cfg(any(test, feature = "private-test-util"))]
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[doc(hidden)]
    #[must_use]
    pub fn fake(
        bucket_upper_bounds: &'static [Magnitude],
        bucket_counts: Vec<u64>,
        overflow_bucket_count: u64,
    ) -> Self {
        assert_eq!(bucket_counts.len(), bucket_upper_bounds.len());

        #[expect(
            clippy::indexing_slicing,
            reason = "Windows guarantee that both indexed elements are in bounds."
        )]
        {
            assert!(
                bucket_upper_bounds
                    .windows(2)
                    .all(|window| window[0] < window[1])
            );
        }

        assert!(!bucket_upper_bounds.contains(&Magnitude::MAX));

        Self {
            magnitudes: bucket_upper_bounds,
            counts: bucket_counts.into_boxed_slice(),
            overflow_bucket_count,
        }
    }
}

/// Target width balances visible relative differences against terminal readability.
///
/// Integer quantization can produce shorter or longer bars.
const TARGET_HISTOGRAM_BAR_WIDTH_CHARS: u64 = 50;

/// Ensures that a nonempty histogram remains renderable when its largest bucket is small.
const MIN_OBSERVATIONS_PER_BAR_CHAR: u64 = 1;

/// Uses the conventional textual representation for an unbounded upper range.
const PLUS_INFINITY_BOUND_LABEL: &str = "+inf";

/// A chunk exceeds the target bar width so ordinary bars require one writer call.
///
/// Low- and high-cardinality rendering benchmarks show that per-glyph writer calls materially
/// increase instruction counts, so the formatter retains this allocation-free specialization.
const HISTOGRAM_BAR_CHUNK: &str = concat!(
    "∎∎∎∎∎∎∎∎",
    "∎∎∎∎∎∎∎∎",
    "∎∎∎∎∎∎∎∎",
    "∎∎∎∎∎∎∎∎",
    "∎∎∎∎∎∎∎∎",
    "∎∎∎∎∎∎∎∎",
    "∎∎∎∎∎∎∎∎",
    "∎∎∎∎∎∎∎∎",
);

/// Makes relative bucket counts easy to compare in a Unicode-capable terminal.
const HISTOGRAM_BAR_CHAR: char = '∎';

fn decimal_width(value: u64) -> usize {
    let width = value
        .checked_ilog10()
        .unwrap_or(0)
        .checked_add(1)
        .expect("a u64 decimal width fits in u32");
    usize::try_from(width).expect("a u64 decimal width fits in usize")
}

fn magnitude_width(value: Magnitude) -> usize {
    decimal_width(value.unsigned_abs())
        .checked_add(usize::from(value.is_negative()))
        .expect("an i64 decimal width fits in usize")
}

impl Display for Histogram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (widest_upper_bound, widest_count) = self.buckets().fold(
            (0, 0),
            |(upper_bound_width, count_width), (magnitude, count)| {
                let magnitude_width = if magnitude == Magnitude::MAX {
                    PLUS_INFINITY_BOUND_LABEL.len()
                } else {
                    magnitude_width(magnitude)
                };

                (
                    cmp::max(upper_bound_width, magnitude_width),
                    cmp::max(count_width, decimal_width(count)),
                )
            },
        );

        let histogram_scale = HistogramScale::new(self);

        for (magnitude, count) in self.buckets() {
            if magnitude == Magnitude::MAX {
                write!(
                    f,
                    "value <= {PLUS_INFINITY_BOUND_LABEL:>widest_upper_bound$} \
                     [ {count:>widest_count$} ]: "
                )?;
            } else {
                write!(
                    f,
                    "value <= {magnitude:>widest_upper_bound$} \
                     [ {count:>widest_count$} ]: "
                )?;
            }

            histogram_scale.write_bar(count, f)?;
            writeln!(f)?;
        }

        Ok(())
    }
}

/// Scales histogram counts into readable Unicode bars.
#[derive(Debug)]
struct HistogramScale {
    /// Number of observations represented by one bar character.
    observations_per_char: NonZero<u64>,
}

impl HistogramScale {
    fn new(histogram: &Histogram) -> Self {
        let max_count = histogram
            .counts()
            .max()
            .expect("the synthesized overflow bucket guarantees a nonempty histogram");

        #[expect(
            clippy::integer_division,
            reason = "Quantization may make bars shorter or longer than the target width."
        )]
        let observations_per_char = NonZero::new(cmp::max(
            max_count / TARGET_HISTOGRAM_BAR_WIDTH_CHARS,
            MIN_OBSERVATIONS_PER_BAR_CHAR,
        ))
        .expect("the minimum observations per character is nonzero");

        Self {
            observations_per_char,
        }
    }

    fn write_bar(&self, count: u64, f: &mut impl Write) -> fmt::Result {
        let histogram_bar_width = count
            .checked_div(self.observations_per_char.get())
            .expect("the observations-per-character divisor is nonzero");
        let mut remaining_width = usize::try_from(histogram_bar_width)
            .expect("histogram scaling keeps bar widths within the usize range");
        let bytes_per_char = HISTOGRAM_BAR_CHAR.len_utf8();
        let chars_per_chunk = HISTOGRAM_BAR_CHUNK
            .len()
            .checked_div(bytes_per_char)
            .expect("the histogram bar character width is nonzero");

        while remaining_width >= chars_per_chunk {
            f.write_str(HISTOGRAM_BAR_CHUNK)?;
            remaining_width = remaining_width
                .checked_sub(chars_per_chunk)
                .expect("the written chunk is no wider than the remaining bar");
        }

        let remaining_bytes = remaining_width
            .checked_mul(bytes_per_char)
            .expect("the remainder is bounded by the histogram chunk");
        let remainder = HISTOGRAM_BAR_CHUNK
            .get(..remaining_bytes)
            .expect("the remainder ends at a histogram character boundary");
        f.write_str(remainder)?;

        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "Panicking is acceptable in tests.")]

    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::sync::{Arc, Barrier};
    use std::thread;

    use static_assertions::assert_impl_all;
    use testing::{assert_panics, with_watchdog};

    use super::*;
    use crate::{LocalEventRegistry, ObservationBagSync, Observations};

    assert_impl_all!(Report: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(EventMetrics: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(Histogram: UnwindSafe, RefUnwindSafe);

    #[test]
    fn histogram_properties_reflect_reality() {
        let magnitudes = &[-5, 1, 10, 100];
        let counts = &[66, 5, 3, 2];

        let histogram = Histogram {
            magnitudes,
            counts: Vec::from(counts).into_boxed_slice(),
            overflow_bucket_count: 1,
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
        // This allows for UTF-8 bar bytes, labels, and integer quantization overshoot without
        // duplicating the rendering algorithm in the test.
        const MAX_LINE_BYTES_PER_TARGET_BAR_CHAR: u64 = 5;

        let magnitudes = &[-5, 1, 10, 100];
        let counts = &[666666, 5, 3, 2];

        let histogram = Histogram {
            magnitudes,
            counts: Vec::from(counts).into_boxed_slice(),
            overflow_bucket_count: 1,
        };

        let mut output = String::new();
        write!(&mut output, "{histogram}").unwrap();

        println!("{output}");

        assert!(output.contains("value <=   -5 [ 666666 ]: "));
        assert!(output.contains("value <=    1 [      5 ]: "));
        assert!(output.contains("value <=   10 [      3 ]: "));
        assert!(output.contains("value <=  100 [      2 ]: "));
        assert!(output.contains("value <= +inf [      1 ]: "));

        let max_line_bytes =
            usize::try_from(TARGET_HISTOGRAM_BAR_WIDTH_CHARS * MAX_LINE_BYTES_PER_TARGET_BAR_CHAR)
                .unwrap();

        for line in output.lines() {
            assert!(line.len() < max_line_bytes);
        }
    }

    #[test]
    fn histogram_magnitude_width_accounts_for_digits_and_sign() {
        // Multiple digits and opposite signs make both parts of the width observable.
        assert_eq!(magnitude_width(12_345), 5);
        assert_eq!(magnitude_width(-12_345), 6);
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
            overflow_bucket_count: 1,
        };

        let event_metrics = EventMetrics {
            name: event_name.clone().into(),
            count,
            sum,
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
            overflow_bucket_count: 1,
        };

        let event_metrics = EventMetrics {
            name: event_name.clone().into(),
            count,
            sum,
            histogram: Some(histogram),
        };

        let mut output = String::new();
        write!(&mut output, "{event_metrics}").unwrap();

        println!("{output}");

        assert!(output.contains(&event_name));
        assert!(output.contains(&count.to_string()));
        assert!(output.contains(&sum.to_string()));
        assert!(output.contains(&mean.to_string()));
        assert!(output.contains("value <= +inf [ 1 ]: "));
    }

    #[test]
    fn report_fake_sorts_unsorted_events_by_name() {
        let later_event = EventMetrics {
            name: "later_event".to_string().into(),
            count: 10,
            sum: Magnitude::from(100),
            histogram: None,
        };

        let earlier_event = EventMetrics {
            name: "earlier_event".to_string().into(),
            count: 5,
            sum: Magnitude::from(50),
            histogram: None,
        };

        let report = Report::fake(vec![later_event, earlier_event]);
        let event_names = report
            .events()
            .map(|event| event.name().as_ref())
            .collect::<Vec<_>>();

        assert_eq!(event_names, ["earlier_event", "later_event"]);
    }

    #[test]
    fn collection_remains_valid_during_concurrent_observation() {
        with_watchdog(|| {
            const EVENT_NAME: &str = "concurrent_report_collection";
            const OBSERVATION_COUNT: usize = 16;
            const OBSERVATION_MAGNITUDE: Magnitude = 3;

            let registry = GlobalEventRegistry::new();
            let checkpoint = Barrier::new(2);

            thread::scope(|scope| {
                let worker = scope.spawn(|| {
                    let observations = Arc::new(ObservationBagSync::new(&[]));
                    let local_registry = LocalEventRegistry::new(&registry);
                    local_registry.register(EVENT_NAME.into(), Arc::clone(&observations));

                    checkpoint.wait();
                    for _ in 0..OBSERVATION_COUNT {
                        observations.insert(OBSERVATION_MAGNITUDE, 1);
                    }
                    checkpoint.wait();

                    drop(local_registry);
                });

                checkpoint.wait();
                let concurrent_report = Report::collect_from(&registry);
                checkpoint.wait();
                worker.join().unwrap();

                let metrics = concurrent_report
                    .events()
                    .find(|event| event.name() == EVENT_NAME)
                    .unwrap();
                assert!(metrics.count() <= OBSERVATION_COUNT as u64);
            });

            let report_after_teardown = Report::collect_from(&registry);
            let metrics = report_after_teardown
                .events()
                .find(|event| event.name() == EVENT_NAME)
                .unwrap();
            assert_eq!(metrics.count(), OBSERVATION_COUNT as u64);
            assert_eq!(
                metrics.sum(),
                Magnitude::try_from(OBSERVATION_COUNT).unwrap() * OBSERVATION_MAGNITUDE
            );
        });
    }

    #[test]
    fn collection_rejects_incompatible_archived_configurations() {
        with_watchdog(|| {
            const EVENT_NAME: &str = "incompatible_archived_report";
            const FIRST_BUCKETS: &[Magnitude] = &[10];
            const SECOND_BUCKETS: &[Magnitude] = &[10, 20];

            let registry = GlobalEventRegistry::new();
            thread::scope(|scope| {
                for buckets in [FIRST_BUCKETS, SECOND_BUCKETS] {
                    let registry = &registry;
                    scope
                        .spawn(move || {
                            let observations = Arc::new(ObservationBagSync::new(buckets));
                            let local_registry = LocalEventRegistry::new(registry);
                            local_registry.register(EVENT_NAME.into(), observations);
                        })
                        .join()
                        .unwrap();
                }
            });

            assert_panics(|| {
                _ = Report::collect_from(&registry);
            });

            let mut retained_configurations = 0;
            registry.inspect(|observation_bags| {
                retained_configurations += usize::from(observation_bags.contains_key(EVENT_NAME));
            });
            assert_eq!(retained_configurations, 2);
        });
    }

    #[test]
    fn report_display_contains_expected_events() {
        let event1 = EventMetrics {
            name: "event1".to_string().into(),
            count: 10,
            sum: Magnitude::from(100),
            histogram: None,
        };

        let event2 = EventMetrics {
            name: "event2".to_string().into(),
            count: 5,
            sum: Magnitude::from(50),
            histogram: None,
        };

        let report = Report {
            events: vec![event1, event2].into_boxed_slice(),
        };

        let mut output = String::new();
        write!(&mut output, "{report}").unwrap();

        println!("{output}");

        assert!(output.contains("event1"));
        assert!(output.contains("event2"));
    }

    #[test]
    fn event_displayed_as_counter_if_unit_values_and_no_histogram() {
        let counter = EventMetrics {
            name: "test_event".to_string().into(),
            count: 100,
            sum: Magnitude::from(100),
            histogram: None,
        };

        let not_counter = EventMetrics {
            name: "test_event".to_string().into(),
            count: 100,
            sum: Magnitude::from(200),
            histogram: None,
        };

        let also_not_counter = EventMetrics {
            name: "test_event".to_string().into(),
            count: 100,
            sum: Magnitude::from(100),
            histogram: Some(Histogram {
                magnitudes: &[],
                counts: Box::new([]),
                overflow_bucket_count: 100,
            }),
        };

        let still_not_counter = EventMetrics {
            name: "test_event".to_string().into(),
            count: 100,
            sum: Magnitude::from(200),
            histogram: Some(Histogram {
                magnitudes: &[],
                counts: Box::new([]),
                overflow_bucket_count: 200,
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
        let histogram = Histogram {
            magnitudes: &[1, 2, 3],
            counts: Box::new([0, 0, 0]),
            overflow_bucket_count: 0,
        };

        let histogram_scale = HistogramScale::new(&histogram);

        let mut output = String::new();
        histogram_scale.write_bar(0, &mut output).unwrap();

        assert_eq!(output, "");
    }

    #[test]
    fn histogram_scale_small() {
        let histogram = Histogram {
            magnitudes: &[1, 2, 3],
            counts: Box::new([1, 2, 3]),
            overflow_bucket_count: 0,
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
        let histogram = Histogram {
            magnitudes: &[1, 2, 3],
            counts: Box::new([
                TARGET_HISTOGRAM_BAR_WIDTH_CHARS + 1,
                TARGET_HISTOGRAM_BAR_WIDTH_CHARS + 1,
                TARGET_HISTOGRAM_BAR_WIDTH_CHARS + 1,
            ]),
            overflow_bucket_count: 0,
        };

        let histogram_scale = HistogramScale::new(&histogram);

        let mut output = String::new();

        histogram_scale
            .write_bar(TARGET_HISTOGRAM_BAR_WIDTH_CHARS + 1, &mut output)
            .unwrap();
        assert_eq!(
            output,
            "∎".repeat(usize::try_from(TARGET_HISTOGRAM_BAR_WIDTH_CHARS + 1).unwrap())
        );
    }

    #[test]
    fn histogram_scale_bar_spans_chunks() {
        let histogram_scale = HistogramScale {
            observations_per_char: NonZero::new(1).unwrap(),
        };
        // One extra character exercises both the full-chunk and remainder writes.
        let count = u64::try_from(HISTOGRAM_BAR_CHUNK.chars().count()).unwrap() + 1;
        let mut output = String::new();

        histogram_scale.write_bar(count, &mut output).unwrap();

        assert_eq!(output, "∎".repeat(usize::try_from(count).unwrap()));
    }

    #[test]
    fn histogram_scale_large_exact() {
        let histogram = Histogram {
            magnitudes: &[1, 2, 3],
            counts: Box::new([
                79,
                TARGET_HISTOGRAM_BAR_WIDTH_CHARS * 100,
                TARGET_HISTOGRAM_BAR_WIDTH_CHARS * 1000,
            ]),
            overflow_bucket_count: 0,
        };

        let histogram_scale = HistogramScale::new(&histogram);

        let mut output = String::new();

        histogram_scale.write_bar(0, &mut output).unwrap();
        assert_eq!(output, "");
        output.clear();

        histogram_scale
            .write_bar(histogram_scale.observations_per_char.get(), &mut output)
            .unwrap();
        assert_eq!(output, "∎");
        output.clear();

        histogram_scale
            .write_bar(TARGET_HISTOGRAM_BAR_WIDTH_CHARS * 1000, &mut output)
            .unwrap();
        assert_eq!(
            output,
            "∎".repeat(usize::try_from(TARGET_HISTOGRAM_BAR_WIDTH_CHARS).unwrap())
        );
    }

    #[test]
    fn histogram_scale_large_inexact() {
        let histogram = Histogram {
            magnitudes: &[1, 2, 3],
            counts: Box::new([
                79,
                TARGET_HISTOGRAM_BAR_WIDTH_CHARS * 100,
                TARGET_HISTOGRAM_BAR_WIDTH_CHARS * 1000,
            ]),
            overflow_bucket_count: 0,
        };

        let histogram_scale = HistogramScale::new(&histogram);

        let mut output = String::new();

        histogram_scale.write_bar(0, &mut output).unwrap();
        assert_eq!(output, "");
        output.clear();

        histogram_scale
            .write_bar(histogram_scale.observations_per_char.get() - 1, &mut output)
            .unwrap();
        assert_eq!(output, "");
        output.clear();

        histogram_scale
            .write_bar(histogram_scale.observations_per_char.get(), &mut output)
            .unwrap();
        assert_eq!(output, "∎");
        output.clear();

        histogram_scale
            .write_bar(TARGET_HISTOGRAM_BAR_WIDTH_CHARS * 1000 - 1, &mut output)
            .unwrap();
        assert_eq!(
            output,
            "∎".repeat(usize::try_from(TARGET_HISTOGRAM_BAR_WIDTH_CHARS).unwrap() - 1)
        );
    }

    #[test]
    fn event_metrics_display_zero_count_reports_flat_zero() {
        let snapshot = ObservationBagSnapshot {
            count: 0,
            sum: 0,
            bucket_magnitudes: &[],
            bucket_counts: Box::new([]),
        };

        let metrics = EventMetrics::new("zero_event".into(), snapshot);

        let output = format!("{metrics}");

        assert!(output.contains("zero_event: 0"));
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
        assert_eq!(metrics.mean(), 10);
        assert!(metrics.histogram().is_none());
    }

    #[test]
    fn event_metrics_new_non_zero_count_with_buckets() {
        const BUCKET_UPPER_BOUNDS: &[Magnitude] = &[-10, 0, 10, 100];
        const EXPLICIT_BUCKET_COUNTS: [u64; 4] = [2, 3, 4, 5];
        const TOTAL_COUNT: u64 = 20;
        const TOTAL_SUM: Magnitude = 500;
        const EXPECTED_OVERFLOW_COUNT: u64 = TOTAL_COUNT
            - (EXPLICIT_BUCKET_COUNTS[0]
                + EXPLICIT_BUCKET_COUNTS[1]
                + EXPLICIT_BUCKET_COUNTS[2]
                + EXPLICIT_BUCKET_COUNTS[3]);

        let snapshot = ObservationBagSnapshot {
            count: TOTAL_COUNT,
            sum: TOTAL_SUM,
            bucket_magnitudes: BUCKET_UPPER_BOUNDS,
            bucket_counts: Box::new(EXPLICIT_BUCKET_COUNTS),
        };

        let metrics = EventMetrics::new("histogram_event".into(), snapshot);

        assert_eq!(metrics.name(), "histogram_event");
        assert_eq!(metrics.count(), TOTAL_COUNT);
        assert_eq!(metrics.sum(), TOTAL_SUM);
        #[expect(
            clippy::integer_division,
            reason = "The public mean contract truncates an integral quotient toward zero."
        )]
        let expected_mean = TOTAL_SUM / Magnitude::try_from(TOTAL_COUNT).unwrap();
        assert_eq!(metrics.mean(), expected_mean);

        let histogram = metrics.histogram().unwrap();

        let magnitudes: Vec<_> = histogram.magnitudes().collect();
        assert_eq!(magnitudes, vec![-10, 0, 10, 100, Magnitude::MAX]);

        let counts: Vec<_> = histogram.counts().collect();
        assert_eq!(
            counts,
            vec![
                EXPLICIT_BUCKET_COUNTS[0],
                EXPLICIT_BUCKET_COUNTS[1],
                EXPLICIT_BUCKET_COUNTS[2],
                EXPLICIT_BUCKET_COUNTS[3],
                EXPECTED_OVERFLOW_COUNT,
            ]
        );

        let buckets: Vec<_> = histogram.buckets().collect();
        assert_eq!(buckets.len(), 5);
        assert_eq!(buckets[0], (-10, 2));
        assert_eq!(buckets[1], (0, 3));
        assert_eq!(buckets[2], (10, 4));
        assert_eq!(buckets[3], (100, 5));
        assert_eq!(buckets[4], (Magnitude::MAX, 6));
    }

    #[test]
    fn event_metrics_new_wraps_explicit_bucket_total() {
        let snapshot = ObservationBagSnapshot {
            count: 4,
            sum: 0,
            bucket_magnitudes: &[10, 20],
            bucket_counts: Box::new([u64::MAX, 2]),
        };

        let metrics = EventMetrics::new("wrapped_buckets".into(), snapshot);
        let counts = metrics.histogram().unwrap().counts().collect::<Vec<_>>();

        assert_eq!(counts, [u64::MAX, 2, 3]);
    }

    #[test]
    fn event_metrics_new_clamps_logically_torn_overflow_count() {
        let snapshot = ObservationBagSnapshot {
            count: 1,
            sum: 0,
            bucket_magnitudes: &[10],
            bucket_counts: Box::new([2]),
        };

        let metrics = EventMetrics::new("torn_snapshot".into(), snapshot);
        let counts = metrics.histogram().unwrap().counts().collect::<Vec<_>>();

        assert_eq!(counts, [2, 0]);
    }

    #[test]
    fn event_metrics_fake_calculates_mean_correctly() {
        let metrics = EventMetrics::fake("test_event", 10, 100, None);

        assert_eq!(metrics.name(), "test_event");
        assert_eq!(metrics.count(), 10);
        assert_eq!(metrics.sum(), 100);
        assert_eq!(metrics.mean(), 10);
        assert!(metrics.histogram().is_none());
    }

    #[test]
    fn event_metrics_fake_calculates_mean_with_different_values() {
        let metrics = EventMetrics::fake("test_event", 25, 500, None);

        assert_eq!(metrics.mean(), 20);
    }

    #[test]
    fn event_metrics_fake_mean_zero_when_count_zero() {
        let metrics = EventMetrics::fake("test_event", 0, 100, None);

        assert_eq!(metrics.count(), 0);
        assert_eq!(metrics.sum(), 100);
        assert_eq!(metrics.mean(), 0);
    }

    #[test]
    fn event_metrics_fake_mean_truncates_toward_zero() {
        let metrics = EventMetrics::fake("test_event", 3, -10, None);

        assert_eq!(metrics.mean(), -3);
    }

    #[test]
    fn histogram_fake_happy_path() {
        let histogram = Histogram::fake(&[10, 50, 100], vec![3, 7, 2], 5);

        let buckets: Vec<_> = histogram.buckets().collect();
        assert_eq!(
            buckets,
            vec![(10, 3), (50, 7), (100, 2), (Magnitude::MAX, 5)]
        );
    }

    #[test]
    fn histogram_fake_empty_magnitudes_is_allowed() {
        let histogram = Histogram::fake(&[], vec![], 42);

        let buckets: Vec<_> = histogram.buckets().collect();
        assert_eq!(buckets, vec![(Magnitude::MAX, 42)]);
    }

    #[test]
    #[should_panic]
    fn histogram_fake_panics_on_length_mismatch() {
        _ = Histogram::fake(&[10, 50, 100], vec![3, 7], 0);
    }

    #[test]
    #[should_panic]
    fn histogram_fake_panics_on_unsorted_magnitudes() {
        _ = Histogram::fake(&[10, 100, 50], vec![3, 7, 2], 0);
    }

    #[test]
    #[should_panic]
    fn histogram_fake_panics_on_equal_magnitudes() {
        _ = Histogram::fake(&[10, 10, 100], vec![3, 7, 2], 0);
    }

    #[test]
    #[should_panic]
    fn histogram_fake_panics_on_magnitude_max_in_buckets() {
        _ = Histogram::fake(&[10, 50, Magnitude::MAX], vec![3, 7, 2], 0);
    }
}
