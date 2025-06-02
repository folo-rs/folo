//! Example of measuring the magnitude of events using histograms.
//! This is a modified variant of the `nm_basic.rs` example - familiarize with that first.
//! 
//! Histograms are often the most valuable part of the metrics, as they allow you to see
//! the distribution of the event magnitudes, not just the count and average. The distribution
//! of event magnitudes is often the most insightful part of the data.

use nm::{Event, Magnitude, Report};

fn main() {
    // We just process a fixed amount of data here.
    const LARGE_BAGEL_COUNT: usize = 1000;
    const SMALL_BAGEL_COUNT: usize = 1300;
    const LARGE_BAGEL_WEIGHT_GRAMS: i64 = 510;
    const SMALL_BAGEL_WEIGHT_GRAMS: i64 = 180;

    for _ in 0..LARGE_BAGEL_COUNT {
        BAGELS_COOKED_WEIGHT_GRAMS.with(|x| x.observe(LARGE_BAGEL_WEIGHT_GRAMS));
    }

    for _ in 0..SMALL_BAGEL_COUNT {
        BAGELS_COOKED_WEIGHT_GRAMS.with(|x| x.observe(SMALL_BAGEL_WEIGHT_GRAMS));
    }

    let report = Report::collect();
    println!("{report}");
}

const BAGEL_WEIGHT_GRAMS_BUCKETS: &[Magnitude] =
    &[0, 100, 200, 300, 400, 500, 600, 700, 800, 900, 1000];

thread_local! {
    static BAGELS_COOKED_WEIGHT_GRAMS: Event = Event::builder()
        .name("bagels_cooked_weight_grams")
        .histogram(BAGEL_WEIGHT_GRAMS_BUCKETS)
        .build();
}
