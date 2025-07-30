//! Provides efficient mechanisms to capture the current timestamp and measure elapsed time.
//!
//! TODO: More docs here.

mod pal;

mod clock;
mod instant;

pub use clock::*;
pub use instant::*;
