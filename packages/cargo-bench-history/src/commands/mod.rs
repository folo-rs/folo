//! Command handlers, one per subcommand. Each handler is asynchronous and is
//! dispatched from [`run`](crate::run).

mod backfill;
mod collect;
mod import;
mod install;
mod reporting;

pub(crate) use backfill::execute as backfill;
pub(crate) use collect::execute as collect;
pub(crate) use import::execute as import;
pub(crate) use install::execute as install;
pub(crate) use reporting::{analyze, bless, examine, list, prune, unbless};
