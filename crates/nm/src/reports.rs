use std::{
    cmp,
    fmt::{Display, Write},
};

use foldhash::{HashMap, HashMapExt};

use crate::{EventName, GLOBAL_REGISTRY, Magnitude, ObservationBagSnapshot};

/// A human- and machine-readable report about observed events.
///
/// For human-readable output, use the `Display` trait implementation. This is intended
/// for writing to a terminal and uses only the basic ASCII character set.
///
/// For machine-readable output, TODO.
#[derive(Debug)]
pub struct Report {
    event_observations: HashMap<EventName, ObservationBagSnapshot>,
}

impl Report {
    /// Generates a report by collecting all metrics from all events.
    pub fn collect() -> Self {
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

        Self {
            event_observations: event_name_to_merged_snapshot,
        }
    }
}

impl Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Sort by name for consistent output.
        let mut sorted_bags: Vec<_> = self.event_observations.iter().collect();
        sorted_bags.sort_by_key(|(name, _)| *name);

        for (name, snapshot) in sorted_bags {
            writeln!(f, "{name}: {snapshot}")?;
        }

        Ok(())
    }
}

/// We auto-scale histogram bars when rendering the report. This is the number of characters
/// that we use to represent the maximum bucket value in the histogram.
///
/// Histograms may be smaller than this, as well, because one character will never represent
/// less than one event (so if the max value is 3, the histogram render will be 3 characters wide).
const HISTOGRAM_BAR_WIDTH_CHARS: u64 = 50;

impl Display for ObservationBagSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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

        if count_as_magnitude == self.sum && self.bucket_counts.is_empty() {
            // If we observe that only unit magnitude events were recorded and there are no buckets,
            // we treat this event as a bare counter and only emit the count.
            //
            // This is a heuristic: we might be wrong (e.g. 0 + 2 looks like 1 + 1) but given that
            // this is a report for manual parsing, we can afford to be wrong in some cases if it
            // makes the typical case more readable.
            writeln!(f, "{} (counter)", self.count)?;
        } else {
            #[expect(
                clippy::arithmetic_side_effects,
                reason = "count == 0 shunt at the top protects against division by zero"
            )]
            writeln!(
                f,
                "{}; sum {}; avg {}",
                self.count,
                self.sum,
                self.sum.wrapping_div(count_as_magnitude)
            )?;
        }

        // If there is no histogram to report (because there are no buckets defined), we are done.
        if self.bucket_counts.is_empty() {
            return Ok(());
        }

        let mut buckets_cumulative: u64 = 0;

        // We use the maximum bucket value to auto-scale the histogram bars.
        let max_bucket_value = self
            .bucket_counts
            .iter()
            .max()
            .expect("guarded by is_empty() above - at least one bucket exists");

        // Magnitude of None indicates the "+inf" bucket.
        let mut bucket_magnitudes_and_counts = self
            .bucket_counts
            .iter()
            .enumerate()
            .map(|(index, &count)| {
                buckets_cumulative = buckets_cumulative.wrapping_add(count);

                let magnitude = self.bucket_magnitudes.get(index)
                    .expect("observation bag type invariant: bucket counts and bucket magnitudes are same length");

                (Some(magnitude), count)
            })
            .collect::<Vec<_>>();

        // Some values are too large to fit into any bucket, so this is the "leftover" that goes
        // into the last bucket, which is always the "+inf" bucket.
        let plus_infinity_count = self.count.saturating_sub(buckets_cumulative);

        bucket_magnitudes_and_counts.push((None, plus_infinity_count));

        // Each character in the histogram bar represents this many events for auto-scaling
        // purposes. We use integers, so this can suffer from aliasing effects if there are
        // not many events. That's fine - the relative sizes will still be fine and the numbers
        // will give the ground truth even if the rendering is not perfect.
        let count_per_char = cmp::max(
            // The +inf bucket may be the largest bucket, so we need to consider it, too.
            cmp::max(max_bucket_value, &plus_infinity_count) / HISTOGRAM_BAR_WIDTH_CHARS,
            1,
        );

        // We measure the dynamic parts of the string to know how much padding to add.

        // We write the observation counts here (both in measurement phase and when rendering).
        let mut count_str = String::new();

        // What is the widest event count string for any bucket? Affects padding.
        let widest_count = bucket_magnitudes_and_counts.iter().fold(0, |n, b| {
            count_str.clear();
            write!(&mut count_str, "{}", b.1)
                .expect("we expect writing integer to String to be infallible");
            cmp::max(n, count_str.len())
        });

        // We write the bucket upper bounds here (both in measurement phase and when rendering).
        let mut upper_bound_str = String::new();

        // What is the widest upper bound string for any bucket? Affects padding.
        let widest_upper_bound = bucket_magnitudes_and_counts.iter().fold(0, |n, b| {
            upper_bound_str.clear();

            match b.0 {
                Some(le) => write!(&mut upper_bound_str, "{le}")
                    .expect("we expect writing integer to String to be infallible"),
                None => write!(&mut upper_bound_str, "+inf")
                    .expect("we expect writing integer to String to be infallible"),
            }

            cmp::max(n, upper_bound_str.len())
        });

        for (magnitude, count) in bucket_magnitudes_and_counts {
            upper_bound_str.clear();

            match magnitude {
                Some(le) => write!(&mut upper_bound_str, "{le}")?,
                None => write!(&mut upper_bound_str, "+inf")?,
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
