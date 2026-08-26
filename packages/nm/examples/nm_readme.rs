//! Demonstrates the exact usage shown in `README.md`.

use std::time::Duration;

use nm::{Event, Report};

thread_local! {
    static PACKAGES_RECEIVED: Event = Event::builder()
        .name("packages_received")
        .build();

    static PACKAGE_SEND_DURATION_MS: Event = Event::builder()
        .name("package_send_duration_ms")
        .build();
}

fn main() {
    PACKAGES_RECEIVED.with(Event::observe_once);

    // A nonzero sample makes the duration magnitude visible in the rendered report.
    let send_duration = Duration::from_millis(150);
    PACKAGE_SEND_DURATION_MS.with(|event| event.observe_millis(send_duration));

    let report = Report::collect();
    print!("{report}");
}
