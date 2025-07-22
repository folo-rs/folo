//! Mechanisms to greatly simplify multi-threaded benchmark logic.
//!
//! Supports both "same logic on multiple threads concurrently" scenarios, as well as more complex
//! scenarios with different workloads on different threads.

mod run;
mod run_meta;
mod threadpool;

// This is in a separate module because 99% of the time the user never needs to name
// these types, so it makes sense to de-emphasize them in the API documentation.
pub mod builder;

pub use run::*;
pub use run_meta::*;
pub use threadpool::*;
