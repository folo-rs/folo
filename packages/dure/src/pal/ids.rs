//! Opaque PAL handles used by logic.

/// Listener for incoming client connections.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ListenerId(pub u64);

/// One connected client or startup channel.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ConnId(pub u64);

/// App-lifetime job object.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct JobId(pub u64);

/// Supervisor-owned pseudoconsole.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct PtyId(pub u64);

/// Spawned app process waitable.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct AppId(pub u64);
