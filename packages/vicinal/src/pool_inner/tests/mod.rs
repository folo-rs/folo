//! In-process worker-group tests organized by lifecycle boundary.

mod failures;
pub(crate) mod helpers;
mod initialization;
mod shutdown;
mod startup;
mod topology;
mod worker_loop;
