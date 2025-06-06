//! # nm - nanometer
//!
//! Collect metrics about observed events with low collection overhead even in
//! highly multithreaded applications running on 100+ processors.
//!
//! # Collected metrics
//!
//! For each defined event, the following metrics are collected:
//!
//! * Count of observations (`u64`).
//! * Mean magnitude of observations (`i64`).
//! * (Optional) Histogram of magnitudes, with configurable bucket boundaries (`[i64]`).
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
//! Recommended event name format: `big_medium_small_units`
//!
//! The above two events are merely counters, so there are no units in the name.
//!
//! When only an event name is provided to the builder, only the count and mean magnitude of
//! observations will be recorded. If you want to capture more information about the distribution
//! of event magnitudes, you must specify the histogram buckets to use.
//!
//! ```
//! use nm::{Event, Magnitude};
//!
//! const PACKAGE_WEIGHT_GRAMS_BUCKETS: &[Magnitude] =
//!     &[0, 100, 200, 500, 1000, 2000, 5000, 10000];
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
//! Choose the bucket boundaries based on the expected distribution of magnitudes.
//!
//! # Capturing observations
//!
//! To capture an observation, call `observe()` on the event.
//! Different variants of this method are provided to capture observations with different
//! characteristics:
//!
//! ```
//! # use nm::{Event, Magnitude};
//! #
//! # const PACKAGE_WEIGHT_GRAMS_BUCKETS: &[Magnitude] = &[0, 100, 200, 500, 1000, 2000, 5000, 10000];
//! #
//! # thread_local! {
//! #     static PACKAGES_RECEIVED: Event = Event::builder()
//! #         .name("packages_received")
//! #         .build();
//! #
//! #     static PACKAGES_RECEIVED_WEIGHT_GRAMS: Event = Event::builder()
//! #         .name("packages_received_weight_grams")
//! #         .histogram(PACKAGE_WEIGHT_GRAMS_BUCKETS)
//! #         .build();
//! #
//! #     static PACKAGE_SEND_DURATION_MS: Event = Event::builder()
//! #         .name("package_send_duration_ms")
//! #         .build();
//! # }
//! use std::time::Duration;
//!
//! // observe(x) observes an event with a magnitude of `x`.
//! PACKAGES_RECEIVED_WEIGHT_GRAMS.with(|e| e.observe(900));
//!
//! // observe_once() observes an event with a nominal magnitude of 1, to clearly express that
//! // this event has no concept of magnitude and we use 1 as a nominal placeholder.
//! PACKAGES_RECEIVED.with(|e| e.observe_once());
//!
//! // observe_millis(x) observes an event with a magnitude of `x` in milliseconds while
//! // ensuring that any data type conversions respect the crate panic and mathematics policies.
//! let send_duration = Duration::from_millis(150);
//! PACKAGE_SEND_DURATION_MS.with(|e| e.observe_millis(send_duration));
//!
//! // batch(count) allows you to observe `count` occurrences in one call,
//! // each with the same magnitude, for greater efficiency in batch operations.
//! PACKAGES_RECEIVED_WEIGHT_GRAMS.with(|e| e.batch(500).observe(8));
//! PACKAGES_RECEIVED.with(|e| e.batch(500).observe_once());
//! PACKAGE_SEND_DURATION_MS.with(|e| e.batch(500).observe_millis(send_duration));
//! ```
//!
//! ## Observing durations of operations
//!
//! You can efficiently capture the duration of function calls via `observe_duration_millis()`:
//!
//! ```
//! use nm::{Event, Magnitude};
//!
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
//!     CONNECT_TIME_MS.with(|e| e.observe_duration_millis(|| {
//!         do_http_connect();
//!     }));
//! }
//! # http_connect();
//! # fn do_http_connect() {}
//! ```
//!
//! This captures the duration of the function call in milliseconds. The measurement has
//! a platform-defined measurement granularity (typically around 1-20 ms). This means that
//! faster operations may indicate a duration of zero.
//!
//! It is not practical to measure the duration of individual operations at a finer level of
//! precision because the measurement overhead becomes prohibitive. If you are observing
//! operations that last nanoseconds or microseconds, you should only measure them in
//! aggregate (e.g. duration per batch of 10000).
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
//! A report can be inspected to extract the data within and deliver it to an external system,
//! such as an OpenTelemetry exporter for storage in a metrics database.
//!
//! ```
//! use nm::Report;
//!
//! let report = Report::collect();
//!
//! for event in report.events() {
//!     println!(
//!         "Event {} has occurred {} times with a total magnitude of {}",
//!         event.name(),
//!         event.count(),
//!         event.sum()
//!     );
//! }
//! ```
//!
//! Note that the report accumulates data from the start of the process. This means the
//! data does not reset between reports. If you only want to record differences, you need
//! to account for the previous state of the event yourself.
//!
//! # Dynamically registered events
//!
//! It is not strictly required to define events as thread-local statics. You can also create
//! instances of `Event` on the fly using the same `Event::builder()` mechanism. This can be useful
//! if you do not know at compile time which events you will need, such as when creating one event
//! per item defined in a configuration file.
//!
//! Note, however, that each event (each unique event name) can only be registered once per thread.
//! Any attempt to register an event two times with the same name on the same thread will result
//! in a panic.
//!
//! # Panic policy
//!
//! This crate may panic when registering events if an invalid configuration
//! is supplied for the event.
//!
//! This crate will not panic for "mathematical" reasons during observation of events,
//! such as overflow or underflow due to excessively large event counts or magnitudes.
//!
//! # Mathematics policy
//!
//! Attempting to use excessively large values, either instantaneous or cumulative, may result in
//! mangled data. For example, attempting to observe events with magnitudes near `i64::MAX`. There
//! is no guarantee made about what the specific outcome will be in this case (though the panic
//! policy above still applies). Do not stray near `i64` boundaries and you should be fine.
//!
//! # Performance tradeoffs
//!
//! While this crate aims for high performance and high scalability to many processors, it also
//! targets general-purpose usage and therefore must make some tradeoffs for generality and
//! convenience of usage.
//!
//! Some explicit tradeoffs include:
//!
//! * There is some overhead in how the metrics are stored, both in terms of indirection layers and
//!   the use of atomic operations. This is necessary to ensure that metrics can be reported without
//!   any action required from the thread that observed the event (i.e. a "pull" model). Better
//!   performance can be achieved by using a "push" model, where each thread pushes its observations
//!   to a central repository. However, that is not a general-purpose solution as not every thread
//!   is under the control of the application logic and has the capability to "push" data at the
//!   appropriate moments in time.
//! * There is some inherent overhead in using thread-local static variables, both due to the way
//!   Rust implements them and due to how the `Event` type is structured internally to support the
//!   "pull" model of reporting observations. Avoiding thread-local statics can offer better
//!   performance in some cases but requires more elaborate bookkeeping from application code.
//!
//! It is likely possible to achieve better performance with entirely custom logic that avoids these
//! tradeoffs.

mod constants;
mod data_types;
mod event;
mod observations;
mod observe;
mod registries;
mod reports;

pub(crate) use constants::*;
pub use data_types::*;
pub use event::*;
pub(crate) use observations::*;
pub use observe::*;
pub(crate) use registries::*;
pub use reports::*;
