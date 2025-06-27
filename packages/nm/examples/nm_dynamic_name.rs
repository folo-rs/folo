//! Example of observing an event whose name is dynamically constructed at runtime.
//! This is a modified variant of the `nm_basic.rs` example - familiarize with that first.
//!
//! This is useful, for example, if you have a configuration file that provides some
//! data set and you want a separate event for each entry in the data set, named after
//! that entry.

use nm::{Event, Report};

fn main() {
    // We just process a fixed amount of data here.
    const LARGE_BAGEL_COUNT: usize = 1000;
    const SMALL_BAGEL_COUNT: usize = 1300;
    const LARGE_BAGEL_WEIGHT_GRAMS: i64 = 510;
    const SMALL_BAGEL_WEIGHT_GRAMS: i64 = 180;

    // Note that it is not required to place `Event` instances in thread-local static variables,
    // though that is the simplest pattern to use if the event name is known at that point.
    let large_bagel_event = Event::builder()
        .name(format!("bagels_cooked_{LARGE_BAGEL_WEIGHT_GRAMS}"))
        .build();

    let small_bagel_event = Event::builder()
        .name(format!("bagels_cooked_{SMALL_BAGEL_WEIGHT_GRAMS}"))
        .build();

    for _ in 0..LARGE_BAGEL_COUNT {
        large_bagel_event.observe_once();
        BAGELS_COOKED_WEIGHT_GRAMS.with(|x| x.observe(LARGE_BAGEL_WEIGHT_GRAMS));
    }

    for _ in 0..SMALL_BAGEL_COUNT {
        small_bagel_event.observe_once();
        BAGELS_COOKED_WEIGHT_GRAMS.with(|x| x.observe(SMALL_BAGEL_WEIGHT_GRAMS));
    }

    let report = Report::collect();
    println!("{report}");
}

thread_local! {
    static BAGELS_COOKED_WEIGHT_GRAMS: Event = Event::builder()
        .name("bagels_cooked_weight_grams")
        .build();
}
