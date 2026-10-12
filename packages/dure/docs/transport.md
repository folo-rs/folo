# Transport

The channel between a supervisor and its clients. Part of the `dure`
[implementation guide](implementation.md).

## Shape

A per-session named pipe carries a framed protocol containing console bytes,
window-size changes, attach and displacement outcomes, and app exit status.

The framed protocol carries a version. A supervisor stamps the version it
speaks into its session record, and a client refuses a session whose version
differs rather than connecting and negotiating. Builds with matching protocol
versions can resume each other's sessions. Message-layout or message-meaning
changes require a protocol-version increment, so incompatibility is reported
before it can appear as a decode failure mid-relay.
Records written before the version existed are assigned an incompatible
sentinel version, because they carry no evidence that their wire format matches.

Pipe names contain a random nonce. First-instance creation prevents a
pre-existing pipe from silently impersonating the supervisor; the pipe rejects
remote clients and its access control list permits only the creating user. The
client end asks for identification-level impersonation only, so a pipe that is
not the intended supervisor can learn who connected but cannot act as them.

The client-supervisor pipe uses overlapped I/O. The handles supplied to
`CreatePseudoConsole` are restricted to synchronous I/O, so each pseudoconsole
channel is serviced by its own blocking thread and buffer. Process lifetime
stays a waitable handle rather than an I/O completion source.

## Listening and accepting

A listener keeps exactly one unconnected pipe instance posted, and accepting a
client immediately posts the next one. That is what makes a steal possible: a
second client can connect while the first is still relaying.

An accept therefore does two things, and the order matters. The accepted
instance becomes a connection first, because a client already owns the other end
and may have sent its attach; only then is the replacement listener created. A
failure creating that replacement retires both the connection and the listener,
releasing all server-side owners of the accepted endpoint so the peer is not
left waiting on a pipe nobody serves.

## Handle ownership and cancellation

Pipe handles are shared owners. An operation takes a reference out of the pipe
table before releasing the table lock, so tearing a connection or listener down
concurrently cannot invalidate a handle that is still in use, nor let Windows
reuse the handle value for an unrelated object. Teardown cancels the I/O
outstanding on the handle, which is what unblocks a waiting reader, and the
handle is closed once the last operation releases it. The pseudoconsole PAL owns
its host pipe handles the same way, for the same reason.

Cancelling reaches only operations that are already pending, and teardown gets
one attempt: it has already dropped the table's reference, so nothing can find
the handle to cancel it a second time. An operation that started just after that
attempt would therefore wait with nothing left to release it. Starting an
operation and cancelling one are consequently mutually exclusive, so an
operation either becomes pending in time to be cancelled or is refused and
reported as a disconnect. Refusing is only possible where issuing an operation
does not block, which is why it covers the overlapped transport handles and not
the synchronous pseudoconsole ones.

Issuance catches a panic inside the cancellation guard and propagates it only
after unlocking. The cancellation flag is unchanged, so cleanup can still cancel
other outstanding operations.

Each connection keeps its encoding buffer under the whole-frame write lock.
Encoding reuses capacity grown by earlier sends, and the buffer remains borrowed
until the write completes or cancellation has completed.

Frame accumulation is separate from native reads. It requires positive progress,
checks the length before allocating the body and rejects overreported transfers.
The Windows reader retains its event, operation and buffer until completion or
completed cancellation, using one deadline across all frame components.

Because these are stack-allocated overlapped structures paired with an event
handle that is closed on return, no path may abandon an operation the kernel
still owns. Cancellation is a request, not a completion, so a caller giving up
on an operation asks for cancellation and then blocks until the operation
reports back before reclaiming either object. Shared handle ownership is what
makes that wait bounded: the pipe outlives the operation issued on it, so the
cancellation always completes.

Ending a connection is therefore the cancellation primitive, not merely a way to
let the peer notice. Shutdown paths depend on that: a supervisor abandoning a
client that stopped reading has no other way to free the thread blocked writing
to it.

## Queued delivery

Every supervisor-side write to a client is queued and delivered by a thread that
owns that connection. A pipe write blocks while the peer is not draining, so
writing directly would let one wedged client hold whichever supervisor path made
the write: the output pump, the steal that is trying to replace it, the attach
that is acknowledging, or the exit teardown. Queueing confines the block to the
owning thread, and FIFO delivery is also what orders the attach acknowledgement
ahead of the app's output and the exit status behind it.

A client that falls further behind than the per-client backlog cap is
disconnected rather than buffered without bound. `dure` keeps no screen buffer,
so output that cannot be delivered has no later value, and the user recovers the
session with a fresh `dure resume`.

## Timeouts
Connecting is a deadline, not an attempt. A pipe instance the wait reported can
be taken by another client before this one opens it, and the supervisor posts a
fresh instance as soon as it accepts, so a busy instance is retried for as long
as the caller's timeout allows.

A timeout means the caller's own deadline was spent, and nothing else. A pipe
that does not exist is reported as absent, because the command layer turns a
timeout into "timed out connecting to session", which is the wrong thing to tell
someone whose session is gone.

## The in-memory transport

The test double models concurrent listeners, connections, blocked sends, and
queued messages, which is what lets the steal-under-load contract be exercised
without the operating system.

It reads no clock. A bounded wait ends early only when a test arms that
expiry, and an armed failure — an expired accept, an expired receive, a failed
send — is held against the pipe the test named, so an unrelated operation cannot
consume it. A regression therefore fails an assertion straight away instead of
spending a production timeout first.
