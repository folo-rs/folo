//! Command handlers. The `run` and `analyze` commands have full implementations;
//! `install` is a stub that later iterations fill in.

mod install;
mod run;

pub(crate) use crate::analyze::execute as analyze;
pub(crate) use install::execute as install;
pub(crate) use run::execute as run;
