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
failure closes the lifetime job, removes any provisional state, reports the
specific error to `run`, and exits. A client never reports a successful start
for a session that cannot be resumed.

Windows APIs sit behind a PAL so logic does not depend on a real console host,
named pipe, or job object. The PAL is the only place that talks to the operating
system. Logic consumes it through facades that select the real implementation, or
a mock implementation in test builds, matching the workspace
[PAL](../../../docs/pal.md) pattern.

The PAL is sliced by responsibility, at a grain that tests can drive, not as a
1:1 wrap of each Win32 call:

* **Session store** — create, read, list, and delete session records; allocate
  ids; provide the registry root. The real implementation uses
  `%LOCALAPPDATA%\dure\`. The root is supplied by the PAL so tests never touch
  the user's real AppData.
* **Processes** — spawn a console-detached supervisor with job breakaway and an
  explicit inherited-handle list; identify it by pid and process creation time;
  create an app-lifetime job; spawn the app attached to a pseudoconsole;
  terminate a verified process handle; wait for an exit status. Failure to break
  away is a PAL error that `run` surfaces rather than ignoring.
* **Transport** — listen, accept, connect, read and write console bytes and
  window-size messages, disconnect. Steal is "accept a new connection while an
  old one still exists."
* **Local console** — detect whether the client has a console, switch it to a raw
  relay, read input, write output, read window size.
* **Pseudoconsole** — create, resize, close, and the byte handles the supervisor
  relays. Whether a child *sees* a console is not mocked; that is an integration
  concern.

The bounded connect wait reads time through an injected `tick` clock, so unit
tests do not wait on real time.

## Testing

Workspace testing rules in [docs/testing.md](../../../docs/testing.md) apply:
no real-time delays, no hung tests, watchdogs on waits, Miri-clean runs, and
production behavior that does not change under `cfg(test)` except by swapping
PAL implementations.

### Unit tests

Everything above the PAL is unit-tested against mocks. Those tests do not create
windows, pipes, jobs, or child processes, and they remain Miri-compatible.

Covered here:

* **CLI** — parse `run`, `resume`, `resume --id`, `list`, `kill --id`; reject
  `kill` without `--id`; treat the command after `--` as a direct exec.
* **Startup** — publish the record and report the id only after every resource is
  ready; propagate each initialization failure; close the lifetime job and
  remove provisional state on failure.
* **Auto-detect** — no sessions fails; exactly one launch-directory match
  attaches; several matches or a single session in a different directory fall
  through to list-and-prompt; non-terminal stdin without `--id` fails;
  canonicalization and case folding are whatever the PAL's canonicalize returns.
* **Id allocation** — smallest unused positive integer; reuse after a session
  ends; two concurrent allocations cannot receive the same id (the store's
  coordination is part of the PAL contract and is mocked as exclusive).
* **Stale records** — a missing or exited supervisor, or a pid whose process
  creation time no longer matches, is deleted and not shown; `resume` and `kill`
  of that id fail after cleanup. A matching, running process whose connect times
  out is kept; list still shows it and kill still targets it.
* **Supervisor loop** — first attach relays bytes both ways; a second attach
  disconnects the first; window size is applied on attach and on later changes;
  app exit ends the supervisor and delivers the status to an attached client; a
  connect that never completes within the injected clock fails resume without
  deleting the record.
* **Detach** — dropping the client does not close the app's input (no EOF);
  output from the app is still drained so a mock that keeps writing cannot block
  the session.
* **Steal under load** — accept remains possible while the current client's
  relay is stuck. This is a real multithreaded test against mock transport, not
  an OS test, so Miri can check it.
* **Kill** — the process PAL opens the recorded pid, verifies its creation time
  and running state, and terminates that handle; pid reuse cannot terminate the
  replacement process; a missing id fails; auto-detect is not consulted.
* **Non-Windows entry** — the binary/library refuses to run.

The real PAL and the facade pass-throughs are not unit-tested (workspace
facade/mutants skip). A helper child process is not used here.

### Integration tests

Integration tests use the real PAL and a small helper executable that is not
part of the published product. They run on Windows, are ignored under Miri, and
use a temporary registry root. They wait on process and pipe events, never on
sleep. Any wait is wrapped in the workspace watchdog.

The harness starts each `dure` client inside an outer test-owned pseudoconsole,
matching the environment OpenSSH provides and avoiding any dependency on the CI
runner having an interactive console. The supervisor creates the inner
pseudoconsole that hosts the helper.

The helper reports whether it has a console, echoes input, records window size,
exits with a requested status, and stays alive until an explicit quit (so an
accidental EOF on stdin is visible as an unexpected exit).

Covered here:

* **Console, not pipes** — the helper reports that it is attached to a console.
* **Exit status** — `run` of a helper that exits delivers that status to the
  client.
* **Startup failure** — an invalid command returns its specific launch error and
  leaves no session record or process behind.
* **Detach and resume** — start, kill the client, the helper is still running,
  `resume` attaches, input reaches the helper.
* **Steal** — a second client attaches; the first client ends with failure; the
  helper keeps running and talks to the second client.
* **Steal with blocked I/O** — a client that no longer reads cannot stop a new
  client from attaching and exchanging input and output with the helper.
* **Resize** — the helper observes the initial size, the size of a resumed
  client, and later changes while that client remains attached.
* **Ctrl+C** — Ctrl+C reaches the helper without terminating the client or
  supervisor.
* **Detached output** — the helper can write beyond the pseudoconsole pipe
  capacity while no client is attached and still accepts input after resume.
* **List and GC** — a live session appears in `list`; after the helper exits it
  does not.
* **Auto-detect** — unique launch directory resumes; two sessions in the same
  directory do not pick arbitrarily.
* **Kill while detached** — `kill --id` ends the helper and an ordinary child
  process of that helper; a missing id fails.
* **Supervisor loss** — terminating the supervisor outside `dure` also ends the
  helper and its ordinary child processes through the app-lifetime job.
* **Job breakaway** — the supervisor is started from a process in a kill-on-close
  job; destroying that job leaves the supervisor and helper running. This is the
  test that a killed parent job does not end the session.
* **Console detachment** — the supervisor is started from a process attached to
  a disposable pseudoconsole; closing that pseudoconsole leaves the supervisor
  and helper running.
* **Nested session** — a supervisor started by `dure run` inside the helper
  breaks away from the outer app-lifetime job and survives termination of the
  outer session.
* **Launch directory** — a relative helper path is resolved against the launch
  directory, not a later working directory.

Integration tests do not try to prove Copilot UI fidelity, nested-pseudoconsole
cosmetics, or behavior when Windows logs the user off.

Non-Windows CI still runs the unit tests and the refuse-to-run check. It does
not run the Windows integration set.

## SSH survival

Windows OpenSSH places the remote shell in a job that is killed on disconnect.
The supervisor is created with job breakaway and as a detached console process,
so it is neither in that job nor attached to the SSH pseudoconsole. Its explicit
inherited-handle list contains only the startup channel; it does not inherit the
client's standard streams or unrelated handles. Breakaway does not create a new
Windows logon session and does not survive logoff. If the current job denies
breakaway, `dure run` fails rather than starting a session that would die on
disconnect.

After breaking away, the supervisor creates a non-inheritable job object with
kill-on-close enabled. It creates the app suspended, assigns it to that job, and
only then resumes it, so no app process can run outside the lifetime boundary
during startup. Ordinary descendants inherit the job. The job permits explicit
breakaway so a nested `dure run` can create an independent inner supervisor;
other apps that deliberately request breakaway receive the same Windows
behavior.

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

The client-supervisor named pipe uses overlapped I/O and can later be bound to
IOCP without a handle-type change. The handles supplied to
[`CreatePseudoConsole`] are restricted to synchronous I/O, so each
pseudoconsole channel is serviced by its own blocking thread and buffer. An
IOCP-based transport would retain those threads as a synchronous bridge unless
the Windows API gains asynchronous pseudoconsole channels. Process lifetime
stays a waitable handle rather than an I/O completion source.

[`CreatePseudoConsole`]: https://learn.microsoft.com/windows/console/createpseudoconsole

## Session registry

Per-session files under the PAL registry root (by default
`%LOCALAPPDATA%\dure\`) record id, supervisor pid, supervisor process creation
time, pipe name, launch directory, command, and session start time. Liveness and
termination open the pid once and verify the process creation time on that
handle, then confirm that it is still running. A missing, exited, or mismatched
process makes the record stale; failure to inspect the process is an error and
does not delete the record. A connect or pipe failure is not evidence of process
death. Id allocation is filesystem-coordinated so two concurrent `run`
invocations cannot take the same id.

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

## Screen buffer extension

The supervisor is the only process that observes every byte of console output.
A future terminal-state model can consume that stream and emit a complete VT
repaint when a client attaches, before live bytes resume. A bounded ring of raw
VT bytes is not an equivalent snapshot because retained sequences may depend on
earlier state that the ring discarded. An empty snapshot plus a resize is the
same attach sequence with nothing to write.
