//! Demonstrates push publishing of metrics.
//!
//! Push publishing offers lower observation overhead but requires each observing thread to
//! publish its metrics explicitly.
//!
//! Each event can use pull or push publishing. Reports merge published metrics from either model.

use nm::{Event, MetricsPusher, Push, Report};

/// A multi-item workload makes the batch operation visible in the report.
const BATCH_SIZE: usize = 10;

/// A representative nonzero weight makes the reported magnitude easy to inspect.
const BAGEL_WEIGHT_GRAMS: i64 = 100;

fn main() {
    print_report("Before observation", &Report::collect());

    BAGELS_COOKED_WEIGHT_GRAMS.with(|event| {
        event.batch(BATCH_SIZE).observe(BAGEL_WEIGHT_GRAMS);
    });

    print_report("After observation, before publication", &Report::collect());

    METRICS_PUSHER.with(MetricsPusher::push);

    print_report("After publication", &Report::collect());
}

fn print_report(stage: &str, report: &Report) {
    println!("## {stage}");
    print!("{report}");
}

thread_local! {
    static METRICS_PUSHER: MetricsPusher = MetricsPusher::new();

    static BAGELS_COOKED_WEIGHT_GRAMS: Event<Push> = Event::builder()
        .name("bagels_cooked_weight_grams")
        .pusher_local(&METRICS_PUSHER)
        .build();
}
