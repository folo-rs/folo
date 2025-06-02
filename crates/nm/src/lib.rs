//! TODO: Add documentation
//!
//! # nm - nanometer
//!
//! Collects and reports metrics about observed events, intended for use in highly
//! multithreaded logic where metrics collection overhead must be minimized.
//!
//! # Data collection
//!
//! TODO
//!
//! # Timing information
//!
//! TODO
//!
//! # Reporting to terminal
//!
//! TODO
//!
//! # Reporting to external systems
//!
//! TODO
//!
//! # Panic policy
//!
//! This crate may panic on "startup" if invalid configuration is supplied, for example
//! to an `Event::builder()`.
//!
//! This crate will not panic for "mathematical" reasons during observation of events,
//! such as overflow or underflow due to excessively large event counts or magnitudes.
//!
//! # Mathematics policy
//!
//! Attempting to use excessively large values, either instantaneous or cumulative, may result in
//! mangled data. For example, attempting to observe events with magnitudes near `i64::MAX`. There
//! is no guarantee made about what the specific outcome will be in this case. Do not stray near
//! `i64` boundaries and you should be fine.

mod constants;
mod data_types;
mod event;
mod observations;
mod registries;
mod reports;

pub(crate) use constants::*;
pub use data_types::*;
pub use event::*;
pub(crate) use observations::*;
pub(crate) use registries::*;
pub use reports::*;
