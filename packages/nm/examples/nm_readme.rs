//! Example that demonstrates the exact usage shown in the README.md file.
//!
//! This shows how to use the `nm` crate for collecting metrics about observed events.

use std::time::Duration;

use nm::Event;

thread_local! {
    static PACKAGES_RECEIVED: Event = Event::builder()
        .name("packages_received")
        .build();

    static PACKAGE_SEND_DURATION_MS: Event = Event::builder()
        .name("package_send_duration_ms")
        .build();
}

fn main() {
    println!("=== NM README Example ===");

    // observe_once() observes an event with a nominal magnitude of 1
    PACKAGES_RECEIVED.with(Event::observe_once);

    // observe_millis() observes an event with a magnitude in milliseconds
    let send_duration = Duration::from_millis(150);
    PACKAGE_SEND_DURATION_MS.with(|e| e.observe_millis(send_duration));

    // Observe a few more events for demonstration
    PACKAGES_RECEIVED.with(Event::observe_once);
    PACKAGES_RECEIVED.with(Event::observe_once);

    let another_duration = Duration::from_millis(75);
    PACKAGE_SEND_DURATION_MS.with(|e| e.observe_millis(another_duration));

    println!("Observed multiple events - check metrics collection");
    println!("README example completed successfully!");
}
