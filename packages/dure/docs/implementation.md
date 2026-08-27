# dure implementation

The user-visible contract belongs in the package [design](design.md). This guide
follows the workspace rules for
[implementation documentation](../../../docs/implementation.md).

## Process split

The same binary implements both roles. `dure run` spawns `dure` again in
supervisor mode (a hidden subcommand or equivalent), then the original process
stays the client and attaches. Windows has no `exec`.

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
* **Processes** — spawn a supervisor with job breakaway, spawn the app attached
  to a pseudoconsole, probe liveness by pid, terminate by pid, wait for an exit
  status. Failure to break away is a PAL error that `run` surfaces rather than
  ignoring.
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
* **Auto-detect** — no sessions fails; exactly one launch-directory match
  attaches; several matches or a single session in a different directory fall
  through to list-and-prompt; non-terminal stdin without `--id` fails;
  canonicalization and case folding are whatever the PAL's canonicalize returns.
* **Id allocation** — smallest unused positive integer; reuse after a session
  ends; two concurrent allocations cannot receive the same id (the store's
  coordination is part of the PAL contract and is mocked as exclusive).
* **Stale records** — a listed pid that the process PAL reports dead is deleted
  and not shown; `resume` and `kill` of that id fail after cleanup. A live pid
  whose connect times out is kept; list still shows it and kill still targets it.
* **Supervisor loop** — first attach relays bytes both ways; a second attach
  disconnects the first; resize is applied on attach; app exit ends the
  supervisor and delivers the status to an attached client; a connect that never
  completes within the injected clock fails resume without deleting the record.
* **Detach** — dropping the client does not close the app's input (no EOF);
  output from the app is still drained so a mock that keeps writing cannot block
  the session.
* **Steal under load** — accept remains possible while the current client's
  relay is stuck. This is a real multithreaded test against mock transport, not
  an OS test, so Miri can check it.
* **Kill** — terminate-by-pid is invoked; a missing id fails; auto-detect is not
  consulted.
* **Non-Windows entry** — the binary/library refuses to run.

The real PAL and the facade pass-throughs are not unit-tested (workspace
facade/mutants skip). A helper child process is not used here.

### Integration tests

Integration tests use the real PAL and a small helper executable that is not
part of the published product. They run on Windows, are ignored under Miri, and
use a temporary registry root. They wait on process and pipe events, never on
sleep. Any wait is wrapped in the workspace watchdog.

The helper reports whether it has a console, echoes input, records window size,
exits with a requested status, and stays alive until an explicit quit (so an
accidental EOF on stdin is visible as an unexpected exit).

Covered here:

* **Console, not pipes** — the helper reports that it is attached to a console.
* **Exit status** — `run` of a helper that exits delivers that status to the
  client.
* **Detach and resume** — start, kill the client, the helper is still running,
  `resume` attaches, input reaches the helper.
* **Steal** — a second client attaches; the first client ends with failure; the
  helper keeps running and talks to the second client.
* **Resize on attach** — after resume, the helper observes a window-size change.
* **List and GC** — a live session appears in `list`; after the helper exits it
  does not.
* **Auto-detect** — unique launch directory resumes; two sessions in the same
  directory do not pick arbitrarily.
* **Kill while detached** — `kill --id` ends the helper; a missing id fails.
* **Job breakaway** — the supervisor is started from a process in a kill-on-close
  job; destroying that job leaves the supervisor and helper running. This is the
  test that a killed parent job does not end the session.
* **Launch directory** — a relative helper path is resolved against the launch
  directory, not a later working directory.

Integration tests do not try to prove Copilot UI fidelity, nested-pseudoconsole
cosmetics, or behavior when Windows logs the user off.

Non-Windows CI still runs the unit tests and the refuse-to-run check. It does
not run the Windows integration set.

## SSH survival

Windows OpenSSH places the remote shell in a job that is killed on disconnect.
The supervisor is created with job breakaway so it is not in that job. Breakaway
does not create a new Windows logon session and does not survive logoff. If the
current job denies breakaway, `dure run` fails rather than starting a session
that would die on disconnect.

## Accept loop and steal

The supervisor listens for a new client even while it is busy reading and
writing the current one. Steal does not depend on the old connection still being
healthy. Connect attempts time out. A supervisor that is alive as a pid but never
accepts stays listed; resume fails and `kill --id` still targets that pid.

`dure kill --id` terminates the supervisor by pid and does not need that
connection to be healthy.

## Transport

A per-session named pipe carries console bytes plus window-size notifications.
Pipe names are unguessable enough to avoid squatting and are ACL'd to the
creating user.

Named pipes and the pseudoconsole byte handles are created overlapped so they
can later be bound to IOCP without a handle-type change. Anonymous `CreatePipe`
is avoided: those handles cannot be overlapped. Client-side console I/O
(`ReadConsole` / the SSH session's stdin and stdout) is the weaker end for IOCP;
under OpenSSH those streams are often already pipes. Process lifetime stays a
waitable handle (`WaitForSingleObject`), which is not an IOCP completion source.

## Session registry

Per-session files under the PAL registry root (by default
`%LOCALAPPDATA%\dure\`) record id, supervisor pid, pipe name, launch directory,
command, and start time. `list` / `resume` / `kill` drop a record only when the
recorded pid is gone. A connect or pipe failure is not enough. Id allocation is
filesystem-coordinated so two concurrent `run` invocations cannot take the same
id.

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
Tokio and no PTY wrapper crate. The process is synchronous: a small number of
blocking threads and `WaitForMultipleObjects` to copy bytes and accept a new
client. Session records on disk use a serde format already in the workspace.

## Screen buffer extension

The supervisor is the only process that observes every byte of console output. A
terminal grid (or a ring of VT) can be filled from that stream and, on attach,
written to the new client before live bytes. An empty snapshot plus a resize is
the same attach sequence with nothing to write.
