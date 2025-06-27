//! Minimal example of observing simple events and reporting the metrics at exit.
//! This is the same as `nm_basic.rs` but structured as a subdirectory example.

use nm::{Event, Report};

fn main() {
    // We just process a fixed amount of data here.
    const LARGE_BAGEL_COUNT: usize = 1000;
    const SMALL_BAGEL_COUNT: usize = 1300;
    const LARGE_BAGEL_WEIGHT_GRAMS: i64 = 510;
    const SMALL_BAGEL_WEIGHT_GRAMS: i64 = 180;

    for _ in 0..LARGE_BAGEL_COUNT {
        LARGE_BAGELS_COOKED.with(Event::observe_once);
        BAGELS_COOKED_WEIGHT_GRAMS.with(|x| x.observe(LARGE_BAGEL_WEIGHT_GRAMS));
    }

    for _ in 0..SMALL_BAGEL_COUNT {
        SMALL_BAGELS_COOKED.with(Event::observe_once);
        BAGELS_COOKED_WEIGHT_GRAMS.with(|x| x.observe(SMALL_BAGEL_WEIGHT_GRAMS));
    }

    let report = Report::collect();
    println!("{report}");
}

thread_local! {
    static BAGELS_COOKED_WEIGHT_GRAMS: Event = Event::builder()
        .name("bagels_cooked_weight_grams")
        .build();

    static SMALL_BAGELS_COOKED: Event = Event::builder()
        .name("bagels_cooked_small")
        .build();

    static LARGE_BAGELS_COOKED: Event = Event::builder()
        .name("bagels_cooked_large")
        .build();
}
