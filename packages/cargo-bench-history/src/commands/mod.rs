//! Command handlers. Each subcommand has its own module; Phase 0 ships stubs that
//! report the command as recognized but not yet implemented.

mod analyze;
mod install;
mod run;

pub(crate) use analyze::execute as analyze;
pub(crate) use install::execute as install;
pub(crate) use run::execute as run;
