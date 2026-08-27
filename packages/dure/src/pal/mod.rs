//! Platform abstraction layer for `dure`.
//!
//! Sliced by responsibility as described in `docs/implementation.md`. Facades
//! select the real implementation or a test double.

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

pub(crate) use error::*;
pub(crate) use local_console::*;
pub(crate) use processes::*;
pub(crate) use pseudoconsole::*;
pub(crate) use session_store::*;
pub(crate) use store_root::*;
pub(crate) use transport::*;

/// Bundle of PAL facades used by command dispatch.
#[derive(Clone, Debug)]
pub(crate) struct Pal {
    /// Session record store.
    pub store: SessionStoreFacade,
    /// Process and job control.
    pub processes: ProcessesFacade,
    /// Client-supervisor transport.
    pub transport: TransportFacade,
    /// Foreground console.
    pub console: LocalConsoleFacade,
    /// App pseudoconsole.
    pub pty: PseudoconsoleFacade,
}

impl Pal {
    /// Real PAL with an optional store-root override.
    pub(crate) fn target(store_root: Option<std::path::PathBuf>) -> Result<Self, PalError> {
        let root = resolve_store_root(store_root)?;
        Ok(Self {
            store: SessionStoreFacade::target(root),
            processes: ProcessesFacade::target(),
            transport: TransportFacade::target(),
            console: LocalConsoleFacade::target(),
            pty: PseudoconsoleFacade::target(),
        })
    }
}
