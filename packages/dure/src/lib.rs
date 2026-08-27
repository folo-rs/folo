//! Detachable Windows console sessions that survive SSH disconnect.
//!
//! User-visible behavior is documented in `docs/design.md`. Internal architecture
//! is documented in `docs/implementation.md`.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

mod attach;
mod cli;
mod commands;
mod constants;
mod detect;
mod errors;
mod gc;
mod list_fmt;
mod pal;
mod platform;
mod protocol;
mod run;
mod session_id;
mod session_record;
mod supervisor;
mod types;

pub use cli::{Cli, EarlyExit};
pub use run::run;
pub use session_id::SessionId;
pub use types::{Command, Outcome, RunInput};

#[cfg(all(feature = "private-test-util", windows))]
pub mod test_support;

pub(crate) use errors::*;
