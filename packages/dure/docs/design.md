# dure design

`dure` keeps an interactive Windows console process running after the SSH session
that started it disconnects, and reattaches a later SSH session to that same
process. It is a per-app supervisor, not a multiplexed server.

Install with `cargo binstall dure`, matching the other published binaries in this
repository.

## Tenets

* **Survive disconnect, not logoff.** Dropping SSH does not stop the app. A
  Windows logoff or reboot does.
* **One supervisor per app.** There is no shared daemon. Supervisors coordinate
  through per-user files on the machine.
* **Transparent once attached.** After attach, keyboard and console I/O are a
  direct funnel. No prefix key, no in-band detach, no `dure` UI inside the app.
* **Detach is losing the client.** Closing the SSH session, closing the terminal,
  or otherwise killing the foreground `dure` process leaves the supervisor and
  app running.
* **Resume is not a replay.** Attach does not reconstruct prior screen contents.
  The attach path allows a snapshot burst from the supervisor before live bytes;
  this design uses an empty snapshot plus a window-size update.
* **Latest client wins.** A new successful attach becomes the sole live console
  and disconnects any older client. That is how a wedged SSH session is
  displaced. There is no separate `--force` flag.

## Roles

```mermaid
flowchart LR
  terminal[SSH terminal] --> client[dure client]
  client <--> supervisor[dure supervisor]
  supervisor --> app[app]
```

* **App** — the command given to `dure run`.
* **Supervisor** — a hidden `dure` process that owns the app and its console and
  outlives SSH. The user never launches this role directly.
* **Client** — the foreground `dure` in the SSH session (`run` or `resume`) that
  relays the user's console to the supervisor.

`dure run -- copilot.exe` starts a supervisor, starts the app under it, and
attaches this client. If SSH dies, the client dies; the supervisor and app do
not.

## Commands

`dure run -- <command> [args...]` starts a new session in the current directory
and attaches immediately, even if another live session already exists for that
directory. Reconnect is `dure resume`, never an implicit side effect of `run`.
The command is executed directly, not through a shell. The child's working
directory and environment are those of the `dure run` process at start time and
do not update on a later resume. Relative command paths are resolved against
that launch directory.

If the process cannot be kept alive across SSH disconnect, `dure run` fails
instead of starting a session that would die on disconnect.

`dure resume` attaches using auto-detect. `dure resume --id <id>` attaches to
that live session and skips auto-detect.

`dure list` prints live sessions: id, launch directory, command, whether a
client is currently attached, supervisor pid. Stale records of dead supervisors
are discarded when observed and are not listed. Attached means the supervisor
still has a client connection; a hung SSH client may still appear attached.
Resume steals anyway.

`dure kill --id <id>` abruptly terminates the supervisor process for that
session. The app is a child of the supervisor and dies with it. `--id` is
required; kill does not auto-detect. Kill targets the supervisor by process id,
so it still works when the session connection is wedged. An attached client, if
any, sees the relay end. Killing a missing or already-dead id is a failure after
stale-record cleanup.

## Auto-detect

Each session records the canonical absolute path of the directory from which
`dure run` was invoked (the launch directory). The app later changing its own
working directory does not change that record.

`dure resume` with no `--id`:

* If there is no live session, it fails.
* If exactly one live session has a launch directory equal to the current
  directory, it attaches to that session.
* Otherwise it prints the live session list and reads a session id from the
  terminal. Unreadable stdin, empty input, or a non-terminal stdin is a failure;
  the caller uses `--id` instead.

If several live sessions share the current directory, that is not a unique
match; the command lists and asks. It does not pick arbitrarily.

Path comparison uses a canonicalized absolute path. Case folding follows the
actual filesystem for that path rather than an operating-system-wide assumption.

## Session identity

A session has a small positive integer id, unique among live sessions for this
user on this machine, stable until that session ends. New sessions take the
smallest unused positive integer, so ids stay short in `list` output. An id may
be reused after the session that had it has ended.

Sessions do not have names. The launch directory is the grouping key for
auto-resume.

## Attach, detach, steal

The supervisor always accepts a new client, independently of whether a client is
currently attached and independently of whether that client's connection still
looks healthy.

When a new client completes attach, the previous client is disconnected and the
new client becomes the sole relay. The supervisor then applies the new client's
window size to the app console. Many TUIs redraw on resize. It does not restore
scrollback, and an app that does not redraw will show an empty or stale screen
until it paints on its own.

If the supervisor does not accept a connection within a bounded wait, it is
treated as dead. The record is dropped. Starting work again is `dure run`.

While detached, the app's console stays open. Input is not closed (end of stdin
would terminate many apps). Output is drained and discarded so the app cannot
block on a full pipe.

Closing the terminal window detaches. It does not stop the app. Ctrl+C while
attached is delivered to the app, not to the client.

## Console I/O

The app is attached to a Windows console owned by the supervisor, not to
stdin/stdout redirected onto anonymous pipes.

A console is a terminal device: it has a width and height, it can deliver
keystrokes such as Ctrl+C as input, and it accepts the control sequences a TUI
uses to move the cursor, set colors, and repaint the screen. Anonymous pipes are
only byte streams. Programs detect the difference and, on pipes, disable the
TUI, skip color, or refuse to run. Interactive tools such as Copilot CLI need
the console case.

From the app's side this is a console. From the supervisor's side it is bytes
plus window size, which the attached client relays to the user's SSH terminal.
The app's stdout and stderr are one console stream.

SSH already wraps the remote shell in a Windows pseudoconsole. `dure` adds
another around the app. For a VT TUI such as Copilot CLI, that extra layer is
not expected to remove features the same app already has over SSH without
`dure`. What SSH already cannot provide (a real console window, graphics
protocols, console font and selection chrome) stays unavailable.

`dure` writes diagnostics to stderr only while it is not attached. Before
attach, `run` and `resume` print the session id so the user can `kill` or
`resume --id` later. After attach, the funnel is exclusive. When a client is
displaced, it may write one diagnostic and then exit with a failure status.
When the app exits, an attached client exits with the app's status.

Attaching (`run`, `resume`) requires a console. `list` and `kill` do not. The
resume id prompt additionally requires a terminal stdin; without one, `resume`
without `--id` fails.

## Lifetime

| Event | App | Supervisor | Client |
| --- | --- | --- | --- |
| SSH disconnect or client killed | running | running | dead |
| Terminal closed | running | running | dead |
| App exits | dead | exits after cleanup | app status if attached |
| Logoff or reboot | dead | dead | dead |
| New attach | running | running | previous client disconnected |
| `dure kill` | dies with supervisor | abruptly terminated | relay ends if attached |
| Supervisor dies | dead | dead | relay ends / resume fails |

The app is a child of its supervisor. If the supervisor dies, the app dies with
it. Supervisors do not orphan apps.

Sessions are a flat list. A `dure run` issued from inside an attached session
(for example the app is a shell) starts another ordinary session. Killing or
detaching one does not cascade to the other: the inner supervisor has already
broken away, which is the same rule as SSH disconnect applied twice.

A machine configured to log off the user when SSH disconnects makes the product
impossible; `dure` cannot outlive a logoff.

## Isolation

Sessions are per Windows user. Coordination files and the client-supervisor
connection are usable only by the creating user. Another user cannot list or
attach.

Command lines appear in `list` output. Secrets do not belong on the app argv.

## Distribution

Published crate and binary name: `dure`. The crate carries the same
`cargo binstall` URL contract as the other published binaries in this
repository.

Runtime support is Windows only. The crate still compiles on the rest of the
workspace target matrix so CI stays unified; a non-Windows binary exits with an
error.

## Screen contents

Attach does not replay prior output. The supervisor may later keep a terminal
grid and, on attach, write that snapshot to the new client before live bytes.
This design is that sequence with an empty snapshot plus a resize. Clients treat
any output that appears immediately after attach as the live console; they do
not need a distinct replay mode.

## Diagnostics

`--verbose` explains auto-detect: which sessions were considered, which paths
were compared, why a session was chosen or why the command fell through to the
list. Failures before attach go to stderr with a non-zero status.

Internal architecture is documented in the
[implementation guide](implementation.md).
