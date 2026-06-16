//! Command handlers, one per subcommand. Each handler is asynchronous and is
//! dispatched from [`run`](crate::run).

mod backfill;
mod install;
mod run;

pub(crate) use crate::analyze::execute as analyze;
pub(crate) use backfill::execute as backfill;
pub(crate) use install::execute as install;
pub(crate) use run::execute as run_with_target_root;
