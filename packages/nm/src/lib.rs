#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! # nm - nanometer
//!
//! Collect metrics about observed events with low overhead even in
//! highly multithreaded applications running on more than 100 logical processors.
//!
//! Included benchmarks have measured between 2 and 20 nanoseconds per observation,
//! depending on event configuration. Results vary by hardware.
//!
//! # Collected metrics
//!
//! Each defined event collects:
//!
//! * Count of observations (`u64`).
//! * Mean magnitude of observations (`i64`).
//! * An optional histogram of magnitudes with configurable bucket boundaries (`[i64]`).
//!
//! # Defining events
//!
//! Use thread-local static variables to define the events to observe:
//!
//! ```
//! use nm::Event;
//!
//! thread_local! {
//!     static PACKAGES_RECEIVED: Event = Event::builder()
//!         .name("packages_received")
//!         .build();
//!
//!     static PACKAGES_SENT: Event = Event::builder()
//!         .name("packages_sent")
//!         .build();
//! }
//! ```
//!
//! Recommended event name format: `big_medium_small_units`.
//!
//! These counter events have no magnitude unit to include in their names.
//!
//! When only an event name is provided, the event records its count and mean magnitude.
//! Configure histogram buckets to also capture the magnitude distribution.
//!
//! ```
//! use nm::{Event, Magnitude};
//!
//! // Broad ranges keep the example readable while covering typical package weights.
//! const PACKAGE_WEIGHT_GRAMS_BUCKETS: &[Magnitude] = &[0, 100, 200, 500, 1000, 2000, 5000, 10000];
//!
//! thread_local! {
//!     static PACKAGES_RECEIVED_WEIGHT_GRAMS: Event = Event::builder()
//!         .name("packages_received_weight_grams")
//!         .histogram(PACKAGE_WEIGHT_GRAMS_BUCKETS)
//!         .build();
//!
//!     static PACKAGES_SENT_WEIGHT_GRAMS: Event = Event::builder()
//!         .name("packages_sent_weight_grams")
//!         .histogram(PACKAGE_WEIGHT_GRAMS_BUCKETS)
//!         .build();
//! }
//! ```
//!
//! Choose bucket boundaries that distinguish the ranges relevant to the workload.
//!
//! # Capturing observations
//!
//! Use [`Event::observe()`] when each occurrence has a meaningful magnitude:
//!
//! ```
//! # use nm::{Event, Magnitude};
//! #
//! # const PACKAGE_WEIGHT_GRAMS_BUCKETS: &[Magnitude] =
//! #     &[0, 100, 200, 500, 1000, 2000, 5000, 10000];
//! #
//! # thread_local! {
//! #     static PACKAGES_RECEIVED_WEIGHT_GRAMS: Event = Event::builder()
//! #         .name("packages_received_weight_grams")
//! #         .histogram(PACKAGE_WEIGHT_GRAMS_BUCKETS)
//! #         .build();
//! # }
//!
//! // This sample falls within the configured histogram and is easy to locate in a report.
//! const SAMPLE_PACKAGE_WEIGHT_GRAMS: Magnitude = 900;
//!
//! PACKAGES_RECEIVED_WEIGHT_GRAMS.with(|event| event.observe(SAMPLE_PACKAGE_WEIGHT_GRAMS));
//! ```
//!
//! Use [`Event::observe_once()`] for occurrences without a meaningful magnitude:
//!
//! ```
//! use nm::Event;
//!
//! thread_local! {
//!     static PACKAGES_RECEIVED: Event = Event::builder()
//!         .name("packages_received")
//!         .build();
//! }
//!
//! PACKAGES_RECEIVED.with(Event::observe_once);
//! ```
//!
//! Use [`Event::observe_millis()`] to convert a duration into a millisecond magnitude:
//!
//! ```
//! use std::time::Duration;
//!
//! use nm::Event;
//!
//! thread_local! {
//!     static PACKAGE_SEND_DURATION_MS: Event = Event::builder()
//!         .name("package_send_duration_ms")
//!         .build();
//! }
//!
//! // A nonzero sample makes the converted magnitude visible in a report.
//! let send_duration = Duration::from_millis(150);
//! PACKAGE_SEND_DURATION_MS.with(|event| event.observe_millis(send_duration));
//! ```
//!
//! Use [`Event::batch()`] to record occurrences with a common magnitude in one call:
//!
//! ```
//! use nm::{Event, Magnitude};
//!
//! thread_local! {
//!     static PACKAGES_RECEIVED_WEIGHT_GRAMS: Event = Event::builder()
//!         .name("packages_received_weight_grams")
//!         .build();
//! }
//!
//! // A multi-item workload demonstrates that one call records the complete batch.
//! const BATCH_SIZE: usize = 500;
//! // This sample is straightforward to locate in a report.
//! const SAMPLE_PACKAGE_WEIGHT_GRAMS: Magnitude = 900;
//!
//! PACKAGES_RECEIVED_WEIGHT_GRAMS.with(|event| {
//!     event.batch(BATCH_SIZE).observe(SAMPLE_PACKAGE_WEIGHT_GRAMS);
//! });
//! ```
//!
//! A batch can represent repeated occurrences without meaningful magnitudes:
//!
//! ```
//! use nm::Event;
//!
//! thread_local! {
//!     static PACKAGES_RECEIVED: Event = Event::builder()
//!         .name("packages_received")
//!         .build();
//! }
//!
//! // A multi-item workload demonstrates that one call records the complete batch.
//! const BATCH_SIZE: usize = 500;
//!
//! PACKAGES_RECEIVED.with(|event| event.batch(BATCH_SIZE).observe_once());
//! ```
//!
//! A batch can also represent repeated occurrences with a common duration:
//!
//! ```
//! use std::time::Duration;
//!
//! use nm::Event;
//!
//! thread_local! {
//!     static PACKAGE_SEND_DURATION_MS: Event = Event::builder()
//!         .name("package_send_duration_ms")
//!         .build();
//! }
//!
//! // A multi-item workload demonstrates that one call records the complete batch.
//! const BATCH_SIZE: usize = 500;
//! // A nonzero sample makes the converted magnitude visible in a report.
//! let send_duration = Duration::from_millis(150);
//!
//! PACKAGE_SEND_DURATION_MS.with(|event| event.batch(BATCH_SIZE).observe_millis(send_duration));
//! ```
//!
//! ## Observing durations of operations
//!
//! You can efficiently capture the duration of function calls via `observe_duration_millis()`:
//!
//! ```
//! use nm::{Event, Magnitude};
//!
//! // Fine lower ranges and broader upper ranges illustrate latency-oriented boundaries.
//! const CONNECT_TIME_MS_BUCKETS: &[Magnitude] = &[0, 10, 20, 50, 100, 200, 500, 1000];
//!
//! thread_local! {
//!     static CONNECT_TIME_MS: Event = Event::builder()
//!         .name("net_http_connect_time_ms")
//!         .histogram(CONNECT_TIME_MS_BUCKETS)
//!         .build();
//! }
//!
//! pub fn http_connect() {
//!     CONNECT_TIME_MS.with(|event| event.observe_duration_millis(do_http_connect));
//! }
//! # http_connect();
//! # fn do_http_connect() {}
//! ```
//!
//! This captures the duration of the function call in milliseconds using a low-precision
//! clock optimized for high-frequency capture. The measurement has a granularity of
//! roughly 1-20 ms. Durations shorter than the granularity may appear as zero.
//!
//! Measuring individual nanosecond- or microsecond-scale operations would add prohibitive
//! overhead. Measure such operations in sufficiently large batches instead.
//!
//! # Reporting to terminal
//!
//! To collect a report of all observations, call `Report::collect()`. This implements the
//! `Display` trait, so you can print it to the terminal:
//!
//! ```
//! use nm::Report;
//!
//! let report = Report::collect();
//! println!("{report}");
//! ```
//!
//! # Reporting to external systems
//!
//! Inspect a report to export its data to an external system, such as an OpenTelemetry
//! metrics backend.
//!
//! ```
//! use nm::Report;
//!
//! let report = Report::collect();
//!
//! for event in report.events() {
//!     println!(
//!         "Event {}: count {}, total magnitude {}",
//!         event.name(),
//!         event.count(),
//!         event.sum()
//!     );
//! }
//! ```
//!
//! Reports accumulate data from the start of the process and do not reset event metrics.
//! An exporter that needs interval metrics computes differences from its previous report.
//!
//! # Publishing models
//!
//! Event metrics reach reports through a pull or push publishing model:
//!
//! - **Pull publishing:** A report reads the latest metrics from each event. This is the
//!   default and requires no explicit publication.
//! - **Push publishing:** An event records metrics in a thread-local [`MetricsPusher`].
//!   Calling [`MetricsPusher::push()`] publishes them for subsequent reports.
//!
//! Push publishing can lower observation overhead, but metrics remain absent from reports until
//! the observing thread publishes them. The following example shows report contents at each stage:
//!
//! ```
//! use nm::{Event, MetricsPusher, Push, Report};
//!
//! thread_local! {
//!     static HTTP_EVENTS_PUSHER: MetricsPusher = MetricsPusher::new();
//!
//!     static HTTP_CONNECTIONS: Event<Push> = Event::builder()
//!         .name("net_http_connections")
//!         .pusher_local(&HTTP_EVENTS_PUSHER)
//!         .build();
//! }
//!
//! let before_observation = Report::collect();
//! println!("Before observation:");
//! print!("{before_observation}");
//!
//! HTTP_CONNECTIONS.with(Event::observe_once);
//!
//! let before_publication = Report::collect();
//! println!("After observation, before publication:");
//! print!("{before_publication}");
//!
//! HTTP_EVENTS_PUSHER.with(MetricsPusher::push);
//!
//! let after_publication = Report::collect();
//! println!("After publication:");
//! print!("{after_publication}");
//! ```
//!
//! Use push publishing only when every observing thread reliably calls
//! [`MetricsPusher::push()`].
//!
//! Choose the publishing model independently for each event.
//!
//! # Dynamically registered events
//!
//! Events may also be constructed at runtime with [`Event::builder()`]. This supports cases where
//! event names are not known at compile time, such as events derived from configuration entries.
//!
//! Each unique event name can be registered only once per thread. Registering the same name again
//! on that thread panics.
//!
//! # Panic policy
//!
//! Registering an event with an invalid configuration may panic.
//!
//! Observation does not panic because of arithmetic overflow or underflow caused by
//! excessively large event counts or magnitudes.
//!
//! # Mathematics policy
//!
//! Instantaneous or cumulative values near the limits of `i64` may produce unspecified metric
//! values. The panic policy still applies.
//!
//! Behavioral rationale is documented in the package
//! [design][design]. The [implementation guide][implementation] describes the `nm` package family
//! architecture.
//!
//! [design]: https://github.com/folo-rs/folo/blob/main/packages/nm/docs/design.md
//! [implementation]: https://github.com/folo-rs/folo/blob/main/packages/nm/docs/implementation.md

pub use nm_impl::{
    Event, EventBuilder, EventMetrics, EventName, Histogram, Magnitude, MetricsPusher,
    ObservationBatch, Observe, PublishModel, Pull, Push, Report,
};
