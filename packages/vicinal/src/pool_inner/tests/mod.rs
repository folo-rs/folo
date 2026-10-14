//! In-process worker-group tests organized by lifecycle boundary.

mod eager;
mod eager_failures;
mod failures;
pub(crate) mod helpers;
mod initialization;
mod initialization_races;
mod shutdown;
mod startup;
mod topology;
mod worker_loop;
