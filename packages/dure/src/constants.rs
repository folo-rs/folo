//! Literal values that encode design and implementation decisions.

use std::time::Duration;

/// Bound so `resume` fails instead of hanging when the supervisor is alive but
/// not accepting.
///
/// Local named-pipe connect is immediate when the supervisor is listening.
/// The duration is an arbitrary watchdog, chosen to sit well above local IPC
/// and well below a human "this is stuck" wait.
/// Ref: docs/design.md, "Attach, detach, steal"; docs/implementation.md,
/// "Accept loop and steal".
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Upper bound on one framed transport message.
///
/// Ordinary console bursts are kibibytes. This is an arbitrary sanity cap so a
/// corrupt length prefix cannot force a huge allocation.
pub(crate) const MAX_FRAME_LEN: u32 = 1024 * 1024;

/// Subdirectory under the per-user `LocalAppData` known folder that holds
/// session records.
///
/// Ref: docs/implementation.md, "Session store".
#[cfg_attr(
    not(windows),
    expect(
        dead_code,
        reason = "joined onto LocalAppData by the Windows store root"
    )
)]
pub(crate) const STORE_SUBDIR: &str = "dure";

/// Hidden subcommand that `dure run` uses to spawn the supervisor process.
///
/// Windows has no `exec`; the same binary implements both roles.
/// Ref: docs/implementation.md, "Process split".
pub(crate) const SUPERVISOR_COMMAND: &str = "__supervisor";

/// Columns used until the first client attach reports a real size.
///
/// VGA text-mode geometry, the historical Windows console default.
/// Ref: docs/design.md, "Attach, detach, steal".
pub(crate) const DEFAULT_PTY_COLS: u16 = 80;

/// Rows used until the first client attach reports a real size.
///
/// VGA text-mode geometry, the historical Windows console default.
/// Ref: docs/design.md, "Attach, detach, steal".
pub(crate) const DEFAULT_PTY_ROWS: u16 = 24;
