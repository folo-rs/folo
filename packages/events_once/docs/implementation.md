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

## User destructors are callback boundaries

Waker vtable operations and payload destruction may reenter the event or storage owner and may
unwind. The state machine therefore finishes the observable transition and releases any borrow or
lock before invoking them. When an endpoint is ending its lifecycle, it first transfers or
completes storage cleanup, while temporarily owning any waker or payload whose destructor must be
deferred. A panic from that destructor can then propagate without stranding a rented slot.

Terminal value extraction is separate from receiver cancellation. The extraction path handles only
stable terminal states and remains inline with the normal receiver flow. Synchronized cancellation
contains the callback-bearing state handoff and storage release in a cold, out-of-line helper, so
completed-receiver destruction does not inherit the cancellation state machine.

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

## Pools and lakes share the release boundary

Typed event pools use `plurality::Pool<T>` because every slot has one known event type. Event
lakes use `plurality::MultiPool`, which routes allocations by their size and alignment. Payload
types whose complete event representations have compatible layouts can therefore reuse the same
slots without a payload-type registry. Thread-safe lakes serialize allocation through their
existing mutex because `MultiPool` has a single allocator, while local lakes rely on thread
confinement.

Renting detaches a `plurality::Box` from its owner and stores only its pointer in the endpoint
references. Final endpoint cleanup reconstructs that box and returns the slot directly, without
accessing the pool or lake. Plurality keeps the backing storage alive while detached boxes exist,
which lets managed endpoints outlive their pool or lake handle. Raw variants retain their
caller-enforced owner-outlives-endpoints contract.

## Diagnostics are a debug-build concern

The awaiter backtrace that pools and lakes expose for leak investigation exists only in debug
builds, along with a storage-independent registry of live events that makes it reachable. The
registry stores pointers to the type-independent backtrace cell in each event, so one registry can
inspect every payload type in a lake. Managed endpoints share ownership of the registry to keep it
alive after the pool or lake handle is dropped; raw endpoints rely on their existing lifetime
contract.

Allocation and diagnostics use separate synchronization. Snapshotting retains the registry lock
or borrow only while cloning the stored backtraces, then releases it before invoking user code.
Final endpoint cleanup unregisters the backtrace cell before returning the plurality slot, so a
snapshot can never observe storage after it has been released. Keeping the synchronization
separate also permits diagnostic callbacks to rent or release events and to inspect the same owner
again.
