//! Inspects collected metrics for export to an external system.
//!
//! See `nm_basic.rs` for a terminal-oriented report.

use nm::{Event, Report};

fn main() {
    /// Different counts make the exported categories distinguishable.
    const LARGE_BAGEL_COUNT: usize = 1000;
    const SMALL_BAGEL_COUNT: usize = 1300;

    /// Distinct weights make the aggregate magnitude meaningful.
    const LARGE_BAGEL_WEIGHT_GRAMS: i64 = 510;
    const SMALL_BAGEL_WEIGHT_GRAMS: i64 = 180;

    for _ in 0..LARGE_BAGEL_COUNT {
        LARGE_BAGELS_COOKED.with(Event::observe_once);
        BAGELS_COOKED_WEIGHT_GRAMS.with(|event| event.observe(LARGE_BAGEL_WEIGHT_GRAMS));
    }

    for _ in 0..SMALL_BAGEL_COUNT {
        SMALL_BAGELS_COOKED.with(Event::observe_once);
        BAGELS_COOKED_WEIGHT_GRAMS.with(|event| event.observe(SMALL_BAGEL_WEIGHT_GRAMS));
    }

    let report = Report::collect();

    // Inspecting metrics individually supports adapters for external systems.
    for event in report.events() {
        println!(
            "Event {}: count {}, total magnitude {}",
            event.name(),
            event.count(),
            event.sum()
        );

        // Reports are cumulative, so an exporter computes differences from its previous report
        // when it needs interval metrics.
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
