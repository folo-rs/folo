//! Multithreaded variant of `nm_dynamic_name.rs`.
//!
//! Each worker creates its own events because `Event` is single-threaded.

use std::{panic, thread};

use nm::{Event, Report};

fn main() {
    /// The workload stays compact while exercising cross-thread report merging.
    const THREAD_COUNT: usize = 4;

    let workers = std::array::from_fn::<_, THREAD_COUNT, _>(|_| thread::spawn(cook_bagels));

    for worker in workers {
        if let Err(payload) = worker.join() {
            panic::resume_unwind(payload);
        }
    }

    let report = Report::collect();
    println!("{report}");
}

fn cook_bagels() {
    /// Different counts make each dynamically named category distinguishable in the report.
    const LARGE_BAGEL_COUNT: usize = 1000;
    const SMALL_BAGEL_COUNT: usize = 1300;

    /// Distinct weights demonstrate runtime discriminators while retaining a common unit.
    const LARGE_BAGEL_WEIGHT_GRAMS: i64 = 510;
    const SMALL_BAGEL_WEIGHT_GRAMS: i64 = 180;

    let large_bagel_event = Event::builder()
        .name(format!(
            "bagels_cooked_weight_{LARGE_BAGEL_WEIGHT_GRAMS}_grams"
        ))
        .build();

    let small_bagel_event = Event::builder()
        .name(format!(
            "bagels_cooked_weight_{SMALL_BAGEL_WEIGHT_GRAMS}_grams"
        ))
        .build();

    for _ in 0..LARGE_BAGEL_COUNT {
        large_bagel_event.observe_once();
        BAGELS_COOKED_WEIGHT_GRAMS.with(|event| event.observe(LARGE_BAGEL_WEIGHT_GRAMS));
    }

    for _ in 0..SMALL_BAGEL_COUNT {
        small_bagel_event.observe_once();
        BAGELS_COOKED_WEIGHT_GRAMS.with(|event| event.observe(SMALL_BAGEL_WEIGHT_GRAMS));
    }
}

thread_local! {
    static BAGELS_COOKED_WEIGHT_GRAMS: Event = Event::builder()
        .name("bagels_cooked_weight_grams")
        .build();
}
