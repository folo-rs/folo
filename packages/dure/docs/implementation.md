# dure implementation

The user-visible contract belongs in the package [design](design.md). This guide
follows the workspace rules for
[implementation documentation](../../../docs/implementation.md).

## Process split

The same binary implements both roles. `dure run` spawns `dure` again in
supervisor mode (a hidden subcommand or equivalent), then the original process
stays the client and attaches. Windows has no `exec`.

`run` gives the supervisor a private one-shot startup channel. The supervisor
allocates the session id, creates the lifetime job and pseudoconsole, starts the
app, creates the session pipe, and atomically publishes the session record. It
reports the id only after the pipe is accepting connections. An initialization
failure closes the lifetime job, removes any provisional state, reports
`StartupErr` on the startup pipe, and exits. The initiating client treats every
non-`StartupOk` response as a failed start. A client never reports a successful start
for a session that cannot be resumed.

Windows APIs sit behind a PAL so logic does not depend on a real console host,
named pipe, or job object. The PAL is the only place that talks to the operating
system. Logic consumes it through facades that select the real implementation, or
a mock implementation in test builds, matching the workspace
[PAL](../../../docs/pal.md) pattern.

The PAL is sliced by responsibility, at a grain that tests can drive, not as a
1:1 wrap of each Win32 call:

* **Session store** — create, read, list, and delete session records; allocate
  ids; provide the store root. The real implementation uses the per-user
  LocalAppData known folder, subdirectory `dure`. The root is supplied by the
  PAL so tests never touch the user's real store.
* **Processes** — spawn a console-detached supervisor with job breakaway;
  identify it by pid and process creation time; create an app-lifetime job;
  spawn the app attached to a pseudoconsole and assigned to that job at
  creation; terminate a verified process handle; wait for an exit status.
  Failure to break away is a PAL error that `run` surfaces rather than ignoring.
* **Transport** — listen, accept, connect, read and write console bytes and
  window-size messages, disconnect. Steal is "accept a new connection while an
  old one still exists."
* **Local console** — detect whether the client has a console, switch it to a raw
  relay, read input, write output, read window size.
* **Pseudoconsole** — create, resize, close, and the byte handles the supervisor
  relays. Whether a child *sees* a console is not mocked; that is an integration
  concern.

Bounded waits use `CONNECT_TIMEOUT` as a stuck-supervisor watchdog. Unit tests
inject timeout and disconnect through the mock transport instead of waiting on
real time.

## Testing

Workspace testing rules in [docs/testing.md](../../../docs/testing.md) apply:
no real-time delays in unit tests, no hung tests, watchdogs on waits, and
production behavior that does not change under `cfg(test)` except by swapping
PAL implementations.

Logic above the PAL is unit-tested against mocks so it stays Miri-compatible.
Those tests cover command parsing, session discovery and garbage collection,
the supervisor steal loop, and the client attach handshake, including failure
paths that must not delete a live supervisor record.

The real PAL is exercised on Windows through nested-pseudoconsole process tests
with a helper child that is not part of the product. That suite proves the app
sees a console, `run` forwards exit status, and session records disappear when
the app exits. Broader SSH-survival and steal scenarios belong to the same
harness; they wait on process and pipe events inside the workspace watchdog.

Integration tests do not try to prove console-host cosmetics, nested
pseudoconsole rendering, or behavior when Windows logs the user off.

Non-Windows CI runs the unit tests and the refuse-to-run check.

## SSH survival

Windows OpenSSH places the remote shell in a job that is killed on disconnect.
The supervisor is created with job breakaway and as a detached console process,
so it is neither in that job nor attached to the SSH pseudoconsole. It does not
inherit handles; it reconnects to the startup named pipe by name. Breakaway does
not create a new Windows logon session and does not survive logoff. If the
current job denies breakaway, `dure run` fails rather than starting a session
that would die on disconnect.

After breaking away, the supervisor creates a non-inheritable job object with
kill-on-close enabled. The app is created with both that job and the
pseudoconsole in the process-attribute list, so it is born inside the lifetime
boundary and attached to a console. Ordinary descendants inherit the job. The
job permits explicit breakaway so a nested `dure run` can create an independent
inner supervisor; other apps that deliberately request breakaway receive the
same Windows behavior.

## Accept loop and steal

The supervisor listens for a new client even while it is busy reading and
writing the current one. Steal does not depend on the old connection still being
healthy. Connect attempts time out. A supervisor whose recorded process identity
is still alive but never accepts stays listed; resume fails and `kill --id`
still targets it.

`dure kill --id` opens the recorded pid, verifies the process creation time and
running state, and terminates that process handle. It never needs the session
connection.

## Transport

A per-session named pipe carries a framed protocol containing console bytes,
window-size changes, attach and displacement outcomes, and app exit status.
Pipe names contain a random nonce. First-instance creation prevents a
pre-existing pipe from silently impersonating the supervisor; the pipe rejects
remote clients and its access control list permits only the creating user.

The client-supervisor named pipe uses overlapped I/O. The handles supplied to
[`CreatePseudoConsole`] are restricted to synchronous I/O, so each
pseudoconsole channel is serviced by its own blocking thread and buffer.
Process lifetime stays a waitable handle rather than an I/O completion source.

[`CreatePseudoConsole`]: https://learn.microsoft.com/windows/console/createpseudoconsole

## Session store

Per-session files under the PAL store root (by default the LocalAppData known
folder, subdirectory `dure`) record id, supervisor pid, supervisor process
creation time, pipe name, launch directory, command, and session start time.
Liveness and termination open the pid once and verify the process creation time
on that handle, then confirm that it is still running. A missing, exited, or
mismatched process makes the record stale; failure to inspect the process is an
error and does not delete the record. A connect or pipe failure is not evidence
of process death. Id allocation is filesystem-coordinated so two concurrent
`run` invocations cannot take the same id.

## Pseudoconsole

The app is attached with `CreatePseudoConsole` and
`PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE`. From the app's side this is a console;
from the supervisor's side it is a pair of pipes plus resize. Raw anonymous
pipes as the child's stdin/stdout are not an alternative: a TUI would not see a
console.

SSH already wraps the remote shell in a pseudoconsole. `dure` adds a second one
around the app. For a VT TUI such as Copilot CLI, that extra layer is not
expected to remove features the same app already has over SSH without `dure`.
HWND-based console features and terminal graphics protocols remain unavailable,
as they already are under SSH. Occasional resize or line-wrap glitches are in
the same class as SSH plus a pseudoconsole.

The client disables its own Ctrl+C handler and forwards the key. Console close
on the client does not propagate a kill into the supervisor's pseudoconsole.

## Crates and concurrency

CLI and errors follow the other binaries: `clap`, `ohno` with `app-err`,
`mimalloc`. Windows APIs come from the workspace `windows` crate. There is no
Tokio and no PTY wrapper crate. The process is synchronous: dedicated blocking
threads service the pseudoconsole channels, while other threads accept and relay
client connections and wait for process exit. Session records on disk use a
serde format already in the workspace.
