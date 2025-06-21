//! Showcases how to use push-style publishing of metrics.
//!
//! This offers lower measurement overhead but comes with the price of having to explicitly
//! publish the metrics from every thread periodically.
//!
//! You can choose per event and/or per thread which model you use. Data from both regular
//! events and push events will end up merged into the same data set for reporting.

use std::{thread, time::Duration};

use nm::{Event, MetricsPusher, Push, Report};

const BATCH_SIZE: usize = 10;
const BAGEL_WEIGHT_GRAMS: i64 = 100;

fn main() {
    // We start a separate thread to generate the reports.
    thread::spawn(move || {
        loop {
            // Every 3 seconds, we publish a new report to the terminal.
            let report = Report::collect();
            println!("## Latest published data:");
            println!("{report}");

            // Sleep for 3 seconds before collecting the next report.
            thread::sleep(Duration::from_secs(3));
        }
    });

    // On the main thread, we process batches of events.
    // Each batch consists of cooking 10 bagels, one per second.
    loop {
        println!("Cooking a batch of {BATCH_SIZE} bagels...");

        for _ in 0..BATCH_SIZE {
            thread::sleep(Duration::from_secs(1));
            BAGELS_COOKED_WEIGHT_GRAMS.with(|x| x.observe(BAGEL_WEIGHT_GRAMS));
        }

        // We only push the metrics between each batch to minimize the publishing overhead.
        // Observe that the report publishing thread only sees the updates after the batch is done.
        METRICS_PUSHER.with(MetricsPusher::push);
    }
}

thread_local! {
    static METRICS_PUSHER: MetricsPusher = MetricsPusher::new();

    static BAGELS_COOKED_WEIGHT_GRAMS: Event<Push> = Event::builder()
        .name("bagels_cooked_weight_grams")
        .pusher_local(&METRICS_PUSHER)
        .build();
}
