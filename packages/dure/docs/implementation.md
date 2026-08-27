# dure implementation

The user-visible contract belongs in the package [design](design.md). This guide
follows the workspace rules for
[implementation documentation](../../../docs/implementation.md).

## Process split

The same binary implements both roles. `dure run` spawns `dure` again in
supervisor mode (a hidden subcommand or equivalent), then the original process
stays the client and attaches. Windows has no `exec`.

Windows APIs sit behind a PAL so tests can mock process creation, console
relaying, and the session registry without talking to a real console host.

## SSH survival

Windows OpenSSH places the remote shell in a job that is killed on disconnect.
The supervisor is created with job breakaway so it is not in that job. Breakaway
does not create a new Windows logon session and does not survive logoff. If the
current job denies breakaway, `dure run` fails rather than starting a session
that would die on disconnect.

## Accept loop and steal

The supervisor listens for a new client even while it is busy reading and
writing the current one. Steal does not depend on the old connection still being
healthy. Connect attempts time out; a supervisor that is alive as a pid but
never accepts is treated as dead.

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

Per-session files under `%LOCALAPPDATA%\dure\` record id, supervisor pid, pipe
name, launch directory, command, and start time. `list` / `resume` probe
liveness (pid plus pipe) and delete stale files. Id allocation is
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
