//! Literal values that encode design and implementation decisions.

use std::time::Duration;

/// Bound so `resume` fails instead of hanging when the supervisor is alive but
/// not accepting.
///
/// Local named-pipe connect is expected to complete immediately when the
/// supervisor is listening; this cap is a stuck-supervisor watchdog, not a
/// network round-trip budget.
/// Ref: docs/design.md, "Attach, detach, steal"; docs/implementation.md,
/// "Accept loop and steal".
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Upper bound on one framed transport message.
///
/// Console relay chunks are far smaller. This is a sanity cap against a corrupt
/// or hostile peer filling memory.
pub(crate) const MAX_FRAME_LEN: u32 = 1024 * 1024;

/// Subdirectory under the per-user `LocalAppData` known folder that holds session
/// records.
///
/// Ref: docs/implementation.md, "Session store".
pub(crate) const STORE_SUBDIR: &str = "dure";

/// Hidden subcommand that `dure run` uses to spawn the supervisor process.
///
/// Windows has no `exec`; the same binary implements both roles.
/// Ref: docs/implementation.md, "Process split".
pub(crate) const SUPERVISOR_COMMAND: &str = "__supervisor";
