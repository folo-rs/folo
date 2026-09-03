# dure design

`dure` keeps an interactive Windows console process running after the terminal
that started it goes away, and reattaches a later terminal to that same process.
It is a per-app supervisor, not a multiplexed server.

Closing a terminal window, dropping an SSH connection, and killing the
foreground process are one event to `dure`: the client is gone and the app keeps
running. SSH is the mainstream case, not a privileged one.

## Tenets

* **Survive losing the terminal, not logoff.** A closed window, a dropped SSH
  connection, or a killed client does not stop the app. A Windows logoff or
  reboot does.
* **One supervisor per app.** There is no shared daemon. Supervisors coordinate
  through per-user files on the machine.
* **Transparent once attached.** After attach, keyboard and console I/O are a
  direct funnel, and so are the terminal attributes around them: window size and
  control sequences pass through in both directions rather than being
  interpreted. No prefix key, no in-band detach, no `dure` UI inside the app.
* **Detach is losing the client.** Anything that kills the foreground `dure`
  process leaves the supervisor and app running.
* **Resume is not a replay.** Attach does not reconstruct prior screen contents.
  The new client sees an empty screen, then live bytes. The exception is the
  app's opening output, which is held for the first client.
* **Latest client wins.** A new successful attach becomes the sole live console
  and disconnects any older client. That is how a wedged client is displaced.
  There is no separate `--force` flag.

## Roles

```mermaid
flowchart LR
  terminal[terminal] --> client[dure client]
  client <--> supervisor[dure supervisor]
  supervisor --> app[app]
```

* **App** — the command given to `dure run`.
* **Supervisor** — a hidden `dure` process that owns the app and its console and
  outlives the terminal. The user never launches this role directly.
* **Client** — the foreground `dure` in the terminal (`run` or `resume`) that
  relays the user's console to the supervisor.

`dure run -- copilot.exe` starts a supervisor, starts the app under it, and
attaches this client. When the terminal goes away the client dies with it; the
supervisor and app do not.

## Commands

`dure run -- <command> [args...]` starts a new session in the current directory
and attaches immediately, even if another live session already exists for that
directory. Reconnect is `dure resume`, never an implicit side effect of `run`.
The command is executed directly, not through a shell. The child's working
directory and environment are those of the `dure run` process at start time and
do not update on a later resume. Relative command paths are resolved against
that launch directory, and a bare command name is looked up on the executable
search path, as a shell would.

A session must be able to outlive the process that launched it. Some launchers
confine their children to a Windows job object that forbids breakaway and kills
everything in it when the launcher exits; `cargo run` is one. Started from such
a launcher, `dure run` fails outright, naming that cause, rather than starting a
session that would die with the launcher. Where the job permits breakaway but an
ancestor job would still end the session, `dure run` says so and starts the
session anyway. Ordinary shells, including the one an SSH session provides,
permit breakaway.

An app that exits before `dure run` has finished attaching still reports its
output and exit status. Only if the `dure run` process itself goes away first is
such a session discarded unreported.

`run` distinguishes creating a session from attaching to it. A startup failure
leaves no live session behind. Once the supervisor and client confirm startup,
an attach failure identifies the session and notes that it may still be running,
so `list`, `resume`, and `kill` remain available.

`dure resume` attaches using auto-detect. `dure resume <id>` attaches to that
live session and skips auto-detect. The hidden `--id <id>` spelling is accepted
as a compatibility alias.

`dure list` prints live sessions: id, whether a client is currently attached,
supervisor pid, how long the session has been running, launch directory, and
command. A record is discarded only when the same supervisor process is gone;
reuse of its numeric process id by another process does not keep the record live.
Attached means the supervisor still has a client connection; a hung client may
still appear attached. Resume steals anyway.

`dure kill --id <id>` abruptly terminates the supervisor process for that
session. The app and its ordinary descendants die with it. `--id` is required;
kill does not auto-detect. Kill targets the recorded supervisor process directly
instead of using the session connection, so it still works when that connection
is wedged. An attached client, if any, sees the relay end. Killing a missing or
already-dead id is a failure after stale-record cleanup.

## Auto-detect

Each session records the canonical absolute path of the directory from which
`dure run` was invoked (the launch directory). The app later changing its own
working directory does not change that record.

`dure resume` with no id:

* If there is no live session, it fails.
* If exactly one live session has a launch directory equal to the current
  directory, it attaches to that session.
* Otherwise it prints the live session list and reads a session id from the
  terminal. Unreadable stdin, empty input, or a non-terminal stdin is a failure;
  the caller passes the session id as an argument instead.

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
new client becomes the sole relay. Input already in flight from the displaced
client does not reach the app. The supervisor then applies the new client's
window size to the app console. Many TUIs redraw on resize. It does not restore
scrollback, and an app that does not redraw will show an empty or stale screen
until it paints on its own.

Concurrent attaches are ordered against each other, so the client that attaches
last is the one left holding the session.

A session is live while the same supervisor process is still running. A record
is dropped only when that process is gone. If the supervisor is running but
does not accept a connection within a bounded wait, resume fails and the
session stays listed; `dure kill --id` still reaches it.

While detached, the app's console stays open. Input is not closed (end of stdin
would terminate many apps). Output is drained and discarded so the app cannot
block on a full pipe.

An attached client that stops reading its connection is disconnected once it
falls far enough behind, rather than being allowed to stall the session. Since
`dure` keeps no screen contents, nothing recoverable is lost; a fresh `dure
resume` takes the session back.

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
plus window size, which the attached client relays to the user's terminal.
The app's stdout and stderr are one console stream.

The terminal the user sits at already presents the app with a console: Windows
Terminal through the console host, an SSH session through a pseudoconsole.
`dure` adds one more around the app. For a VT TUI such as Copilot CLI, that
extra layer is not expected to remove features the same app already has in that
terminal without `dure`. What the terminal itself cannot provide (a real console
window, graphics protocols, console font and selection chrome) stays
unavailable.

`dure` writes diagnostics to stderr only while it is not attached. Before
attach, `run` and `resume` print the session id so the user can `kill` or
`resume <id>` later. After attach, the funnel is exclusive. A displaced client
writes one diagnostic once its relay has ended and exits with a failure status.
When the app exits, an attached client receives the app's remaining output
before the exit status, then exits with that status.

Attaching (`run`, `resume`) requires a console. `list` and `kill` do not. The
resume id prompt additionally requires a terminal stdin; without one, `resume`
without an id fails.

A console is the Windows console-host API: handles, modes, and window size. A
terminal is the visible emulator (Windows Terminal, an SSH client) that renders
VT. `dure` attaches to a console and relays bytes to whatever terminal the user
is already sitting at.

Text keeps its meaning across the relay in both directions. Non-ASCII output —
the box-drawing characters a TUI frames itself with, accented letters, symbols —
reaches the terminal as the app wrote it, and non-ASCII input reaches the app as
the user typed it. `dure` converts the console it attaches to for the duration
of the relay and converts it back on the way out, so a shell that shares that
console before or after a session is unaffected.

## Terminal pass-through

An app under `dure` behaves as though it were talking to the user's terminal
directly. The relay is transparent rather than interpretive: it carries the
terminal's state and the user's intent through to the app's console instead of
deciding what either of them means.

**Window size.** The app always sees the size of the terminal it is currently
being viewed through. The attaching client's size is applied to the app's
console before the relay starts, and a resize while attached is carried through
as it happens, so an app that redraws on resize reflows immediately. A resize is
therefore not a detach-and-resume affair; it works in the middle of a live
session. While no client is attached the console keeps the size the last client
gave it, and the next attach replaces it. This is what makes a session portable
between terminals: resuming from a differently sized window is a resize, not a
broken layout.

**Keyboard.** Keys arrive at the app the way the terminal sent them, including
the ones that are sequences rather than characters — arrows, function keys, Home
and End, and modifier chords. Ctrl+C is delivered to the app rather than acted on
by the client.

**Control sequences.** The app's output is relayed byte for byte, so cursor
movement, colors and styling, the alternate screen buffer, scroll regions, and
window-title sequences all reach the terminal unchanged. `dure` does not parse,
rewrite, or filter this stream.

Mouse reporting is deliberately not carried, and neither are console events that
describe the window rather than the user's input, such as focus changes and menu
activity. An app that would use the mouse in the user's terminal does not get it
under `dure`.

Everything in this section follows from the relay being byte-transparent and
size-aware; none of it is per-application support, so an app that works in the
user's terminal is expected to work under `dure` without knowing `dure` exists.
The limits are the ones already stated under [Console I/O](#console-io): what
the terminal itself cannot provide stays unavailable, and screen contents from
before an attach are not replayed.

## Listing sessions

`list` prints a table: a heading row and one row per live session, columns
separated by a fixed gap and each column as wide as its widest cell, so a
heading sits above the values it names. Column order is the order given under
[Commands](#commands). Cells are never truncated; a long command widens its
column rather than losing text.

Directories print in the form a user would type. Windows extended-length paths
carry a `\\?\` prefix that the shell does not use and that a user cannot paste
back; `list` and `--verbose` show the plain path instead. Matching still uses
the canonical path, so what is displayed is a rendering choice and never changes
which session `resume` finds.

Age is how long the session has been running, measured from when its supervisor
published the session and rendered to the two coarsest units that apply, so a
session started moments ago and one started days ago are told apart at a glance
without the column growing wide enough to push the directory around. It is a
display value: nothing selects, orders, or reaps a session by it, and a machine
whose clock has moved shows a misleading age rather than behaving differently.

The same table is printed when `resume` cannot auto-detect and has to ask which
session to take, so the id being typed is chosen from the same information
`list` gives.

A session is described, never obeyed. A command may have been launched with
arguments carrying control characters, and a terminal would act on them, so they
print in escaped form: a session occupies one row whatever it was launched with,
and listing sessions cannot move the cursor or repaint the screen.

## Lifetime

| Event | App | Supervisor | Client |
| --- | --- | --- | --- |
| Terminal closed, SSH dropped, or client killed | running | running | dead |
| App exits | dead | exits after cleanup | app status if attached |
| Logoff or reboot | dead | dead | dead |
| New attach | running | running | previous client disconnected |
| `dure kill` | dies with supervisor | abruptly terminated | relay ends if attached |
| Supervisor dies | dead | dead | relay ends / resume fails |

The supervisor owns the app and its ordinary descendants. If the supervisor
dies, they die with it. Supervisors do not orphan apps.

Sessions are a flat list. A `dure run` issued from inside an attached session
(for example the app is a shell) starts another ordinary session. Killing or
detaching one does not cascade to the other: the inner supervisor has already
broken away, which is the same rule as losing a terminal, applied twice.

A machine configured to log the user off when their interactive session ends
makes the product impossible; `dure` cannot outlive a logoff.

## Isolation

Sessions are per Windows user. Coordination files and the client-supervisor
connection are usable only by the creating user. Another user cannot list or
attach.

Command lines appear in `list` output. Secrets do not belong on the app argv.

## Distribution

Crate and binary name: `dure`, published to crates.io and installed with
`cargo binstall dure` under the same prebuilt-archive contract as the other
published binaries in this repository. Prebuilt archives exist for Windows only,
so an install on any other target falls back to building from source.

`dure` is a Windows tool. On other targets the binary reports that it is
unsupported and exits with a failure status.

## Screen contents

Attach does not replay prior output. The new client sees an empty screen, then
live bytes. The window size is the attaching client's, so an app that redraws
on resize repaints itself at the right geometry without any replay.

The one exception is the app's opening output. `dure run` starts the app and
only then attaches, so an app that prints immediately would otherwise speak
before it has an audience. That opening output is held for the first client and
delivered to it. Later attaches begin on an empty screen.

## Diagnostics

`--verbose` explains what the command is doing well enough that its decisions
can be reconstructed from the output: where the session store is, which session
records were read, why each was judged live or dead, and what each command then
did with them. For `resume` that includes auto-detect — which launch directories
were compared against the current one, and why a session was chosen or why the
command fell through to the list. For `run` it includes the app and launch
directory, the connection the supervisor was given, the command line it was
spawned with, and whether the session will survive the launcher.

Verbose output is explanatory, never a substitute for the command's own result,
and goes to stderr so it does not contaminate `list` output being read by
something else. It stops at attach: once the funnel is exclusive, the app owns
the screen. Failures before attach go to stderr with a non-zero status.

Internal architecture is documented in the
[implementation guide](implementation.md).
