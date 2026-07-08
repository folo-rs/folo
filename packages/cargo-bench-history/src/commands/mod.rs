//! Command handlers, one per subcommand. Each handler is asynchronous and is
//! dispatched from [`run`](crate::run).

mod backfill;
mod collect;
mod install;

pub(crate) use backfill::execute as backfill;
pub(crate) use cbh_analyze::{analyze, bless, examine, list, prune, unbless};
pub(crate) use collect::execute as collect;
pub(crate) use install::execute as install;
