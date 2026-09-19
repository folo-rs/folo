#![allow(
    clippy::indexing_slicing,
    reason = "Tests mutate known JSON object fixtures; indexing identifies the intended field directly."
)]

mod execution;
mod fake;
mod publication;
mod validation;
