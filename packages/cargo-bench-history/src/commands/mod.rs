//! Command handlers, one per subcommand. Each handler is asynchronous and is
//! dispatched from [`run`](crate::run).

mod backfill;
mod install;
mod run;

pub(crate) use crate::analyze::bless::{bless, unbless};
pub(crate) use crate::analyze::execute as analyze;
pub(crate) use crate::analyze::list::execute as list;
pub(crate) use crate::analyze::prune::execute as prune;
pub(crate) use backfill::execute as backfill;
pub(crate) use install::execute as install;
pub(crate) use run::execute as run;
