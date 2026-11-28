//! In-process tests for policy-selected groups and worker participation.

mod eager;
mod eager_failures;
mod failures;
pub(crate) mod helpers;
mod initialization;
mod initialization_races;
mod policy;
mod shutdown;
mod startup;
mod topology;
mod worker_loop;
