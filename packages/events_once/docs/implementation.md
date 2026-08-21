# events_once implementation

This guide describes the internal architecture of `events_once`: how an event is represented,
how the two endpoints coordinate through it, and where the memory-safety obligations of the
package live. User-visible behavior and its vocabulary are in [design.md](design.md).

## Layers

The package is built from three layers, each with a single responsibility:

* **The event** — the state machine and the storage for the payload and the awaiter. Two
  implementations exist: a thread-safe one that coordinates through atomics, and a
  single-threaded one that coordinates through cells. They implement the same lifecycle and
  differ only where concurrency forces them to.
* **The endpoint reference** — the storage policy that connects an endpoint to its event. It
  converts ownership of the event's storage into the shared references that all event
  operations use, and it performs the storage-specific release. One implementation exists per
  storage strategy: heap-allocated, caller-placed, pool-managed, and raw-pool-managed.
* **The endpoint cores and their wrappers** — the generic sender and receiver logic, which is
  written once against the reference-policy abstraction, plus the thin public wrapper types
  that give each storage strategy its own named endpoint types.

Keeping the cores generic over the reference policy is what allows every storage strategy to
share one state machine implementation, so a lifecycle fix applies to all of them at once.

## The state machine is the single source of truth

`core/state.rs` defines the states and their meaning: which fields are initialized, when the
event has completed, which endpoint owns cleanup, and in what order terminal states become
visible to user callbacks. Both event implementations and all documentation derive their
statements from that definition rather than restating it, because every one of those facts is
load-bearing for memory safety, and independently maintained paraphrases drift apart.

Two properties of the state machine matter beyond the event implementations themselves:

* **Completion is not payload availability.** Both terminal states finish reception, but only
  one of them yields a payload. Everything that reports readiness — synchronous inspection,
  synchronous extraction, and polling — resolves to the same classification.
* **Cleanup ownership is granted, not inferred.** An endpoint learns that it owns cleanup only
  as the result of the transition it performed. No endpoint may derive that right from an
  observation it made earlier, because the other endpoint may act in between.

## Unsafe reference ownership and the release boundary

The endpoint reference is the package's central unsafe abstraction, so its obligations are
stated at the trait rather than rediscovered by each implementation and each caller:

* The **implementer** guarantees event identity and validity: dereferencing yields one specific
  initialized, aligned event that stays valid for as long as the holding endpoint can access it,
  reachable only through shared references, and with its storage released only through the
  trait's release operation. Each storage strategy discharges this once, in one place, using the
  facts of its own storage: an owned allocation, a caller's placement contract, or a pool slot.
* The **caller** guarantees release ownership: it may release the event only after the state
  machine transition that granted it sole cleanup ownership, and it must not access the event
  afterwards. Every release call site in the generic cores names the transition that granted the
  right, so the proof is local to the branch that performs the release.

This split keeps the memory-safety argument auditable: a new storage strategy is forced to
answer the identity-and-validity half, and new endpoint logic is forced to answer the
release-ownership half, with the compiler marking both places.

Because the event exposes interior mutability, the reference policy hands out an `UnsafeCell`
rather than a plain reference. Each core converts that cell into an event reference in exactly
one place, so the aliasing argument — shared access only, with the event synchronizing its own
fields — is made once per core instead of at every operation.

Endpoint references are pointer handles and are never structurally pinned. The abstraction
records this, which lets the receiver cores project a pinned reference to themselves without
unsafe code, and turns any future pin-sensitive state into a compile error rather than a silent
change of an assumption.

## Callback cleanup ownership

The event state machine never destroys an undelivered payload or a receiver-owned waker while it
still needs to access event storage. It moves that value back to the endpoint core after
publishing the terminal state. The core then arms a storage-release guard before running the
destructor.

If the endpoint owns cleanup, the guard releases boxed, placed or pooled storage during
unwinding. If the peer endpoint still owns cleanup, the terminal state directs that peer to
release storage later. This keeps callback execution outside event borrows and pool locks while
preserving exactly one release owner.

## Diagnostics are a debug-build concern

The awaiter backtrace that pools and lakes expose for leak investigation exists only in debug
builds, along with the pool-side registry of live events that makes it reachable. The debug-only
state is released by the same endpoint that releases the event, because event storage may be
reused or freed without dropping the event.
