//! Platform abstraction layer for `dure`.
//!
//! Sliced by responsibility as described in `docs/implementation.md`. Facades
//! select the real implementation or a test double.

mod bundle;
pub(crate) mod error;
pub(crate) mod ids;
pub(crate) mod local_console;
pub(crate) mod processes;
pub(crate) mod pseudoconsole;
#[cfg(windows)]
pub(crate) mod raw_handle;
pub(crate) mod session_store;
pub(crate) mod store_root;
pub(crate) mod transport;

pub(crate) use bundle::Pal;
