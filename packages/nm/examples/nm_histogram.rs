//! Measures event magnitudes with a histogram.
//!
//! See `nm_basic.rs` for the corresponding example without a histogram.
//!
//! Histograms reveal the distribution of event magnitudes in addition to the count and mean.

use nm::{Event, Magnitude, Report};

fn main() {
    /// Different counts make the fixture categories distinguishable in the report.
    const LARGE_BAGEL_COUNT: usize = 1000;
    const SMALL_BAGEL_COUNT: usize = 1300;

    /// The fixture weights occupy separate histogram ranges.
    const LARGE_BAGEL_WEIGHT_GRAMS: i64 = 510;
    const SMALL_BAGEL_WEIGHT_GRAMS: i64 = 180;

    for _ in 0..LARGE_BAGEL_COUNT {
        BAGELS_COOKED_WEIGHT_GRAMS.with(|event| event.observe(LARGE_BAGEL_WEIGHT_GRAMS));
    }

    for _ in 0..SMALL_BAGEL_COUNT {
        BAGELS_COOKED_WEIGHT_GRAMS.with(|event| event.observe(SMALL_BAGEL_WEIGHT_GRAMS));
    }

    let report = Report::collect();
    println!("{report}");
}

/// Evenly spaced boundaries make it easy to locate each fixture weight in the rendered histogram.
const BAGEL_WEIGHT_GRAMS_BUCKETS: &[Magnitude] =
    &[0, 100, 200, 300, 400, 500, 600, 700, 800, 900, 1000];

thread_local! {
    static BAGELS_COOKED_WEIGHT_GRAMS: Event = Event::builder()
        .name("bagels_cooked_weight_grams")
        .histogram(BAGEL_WEIGHT_GRAMS_BUCKETS)
        .build();
}
