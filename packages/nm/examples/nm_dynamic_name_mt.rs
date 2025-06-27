//! This is a multithreaded variant of the `nm_dynamic_name.rs` example, showcasing how to
//! deal with the fact that `Event` instances are single-threaded.

use std::{iter, thread};

use nm::{Event, Report};

fn main() {
    const THREAD_COUNT: usize = 4;

    // Each thread will cook some bagels.
    let threads = iter::repeat_with(|| thread::spawn(cook_bagels))
        .take(THREAD_COUNT)
        .collect::<Vec<_>>();

    for thread in threads {
        thread.join().unwrap();
    }

    let report = Report::collect();
    println!("{report}");
}

fn cook_bagels() {
    const LARGE_BAGEL_COUNT: usize = 1000;
    const SMALL_BAGEL_COUNT: usize = 1300;
    const LARGE_BAGEL_WEIGHT_GRAMS: i64 = 510;
    const SMALL_BAGEL_WEIGHT_GRAMS: i64 = 180;

    // While `Event` instances are single-threaded, we can still use them on multiple threads.
    // Simply create separate instances of `Event` on each thread, with the same configuration.
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
}

thread_local! {
    static BAGELS_COOKED_WEIGHT_GRAMS: Event = Event::builder()
        .name("bagels_cooked_weight_grams")
        .build();
}
