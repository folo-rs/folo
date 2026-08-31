//! Detachable Windows console sessions that outlive the terminal.
//!
//! User-visible behavior is documented in `docs/design.md`. Internal architecture
//! is documented in `docs/implementation.md`.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
// `dure` supervises Windows consoles and has no meaning on other platforms, so
// the whole crate is gated here rather than each module carrying its own
// platform stub (implementation.md, "Platform gate").
#![cfg(windows)]

mod attach;
mod cli;
mod commands;
mod constants;
mod detect;
mod durability;
mod errors;
mod gc;
mod list_fmt;
mod outbox;
mod pal;
mod path_display;
mod protocol;
mod run;
mod session_id;
mod session_record;
mod startup_watch;
mod supervisor;
mod trace;
mod types;
mod wall_clock;

pub use cli::{Cli, EarlyExit};
pub use run::run;
pub use session_id::SessionId;
pub use types::{Command, Outcome, RunInput};

// Helpers that exist only so integration tests can drive the Windows PAL, so
// they are test infrastructure rather than product code.
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(feature = "private-test-util")]
pub mod test_support;

pub(crate) use errors::*;
