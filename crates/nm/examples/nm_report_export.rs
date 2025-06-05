//! Showcases how to inspect the collected metrics for reporting to an external system.
//! This is a variation of the `nm_basic` example - familiarize with that first.

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

    // Instead of printing the report to the terminal, we can inspect it.
    // This allows you to create an adapter for delivering this data to an external system.
    for event in report.events() {
        // Here we can inspect each event and its data and deliver it to an external system.
        // For example purposes, we just print out some info to the terminal here.
        println!(
            "Event {} has occurred {} times with a total magnitude of {}",
            event.name(),
            event.count(),
            event.sum()
        );

        // Note that the report accumulates data from the start of the process. This means the
        // data does not reset between reports. If you only want to record differences, you need
        // to account for the previous state of the event yourself.
    }
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
