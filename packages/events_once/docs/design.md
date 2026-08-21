# events_once design

`events_once` provides one-time events (channels): a sender/receiver pair where the sender
delivers at most one payload and the receiver observes it exactly once, either by awaiting or
by polling. This document covers the user-visible contracts of the package's building blocks
and the vocabulary used to describe them consistently across public documentation.

## Event completion vocabulary

An event receiver is in exactly one of two conditions from the caller's point of view: pending,
or terminal. It is pending until the corresponding sender either delivers a value or disconnects
without delivering one. Once terminal, the receiver either yields the delivered value or reports
that the sender disconnected without sending one; both are terminal outcomes, so "the receiver
is ready" describes reaching either outcome, not specifically that a value is available. Public
documentation and examples use "pending" and "terminal" (or the concrete outcome: value
delivered, or disconnected) rather than any other paraphrase, so that a reader cannot mistake
readiness for the presence of a value.

Awaiting a receiver and calling its `into_value()` method observe the same completion: awaiting
suspends the caller until the receiver becomes terminal, while `into_value()` reports immediately
whether the receiver is already terminal, without suspending. Both report the sender's
disconnection as an error rather than treating it as an absence of a result.

## Pools and lakes: clone-sharing semantics

`EventPool<T>` and `EventLake` are handles to a resource pool that events are rented from. Each
handle can be cheaply cloned. A handle and every value cloned from it — directly or transitively
— share the same backing pool of resources, so an event rented through one clone is returned to
the pool visible through every other clone of that same handle. A separately constructed pool or
lake, even for the same payload type, owns its own independent pool of resources and shares
nothing with any other pool or lake, cloned or not.

Neither a pool nor a lake needs to outlive the events rented from it: the safe variants keep
their backing resources alive for as long as any rented event or any clone of the handle still
exists.

## Raw pools and lakes: an unsafe lifetime tradeoff, not a speed promise

`RawEventPool<T>` and `RawEventLake` offer the same renting API as `EventPool<T>` and
`EventLake`, but do not keep their backing resources alive by reference counting. Instead, the
caller takes on an unsafe obligation: the pool or lake must remain alive, pinned, and undropped
for as long as any event rented from it still exists. In exchange, the caller is freed from
carrying an `Arc`/`Rc` handle to the pool for that purpose and may manage the pool's lifetime
through whatever ownership structure already guarantees the required outlives relationship.

Choosing a raw variant is a decision about who is responsible for proving a lifetime
relationship — the type system through reference counting, or the caller through an unsafe
contract — not a guarantee of lower runtime cost. Callers should reach for the raw variants only
when they already have an independent, easily verified guarantee that the pool or lake outlives
every event rented from it, not in pursuit of an assumed performance advantage.

## Embedding events in a container

An event can be embedded as a field of another object instead of being allocated on the heap, via
`EmbeddedEvent<T>` and `Event::placed()`. This is also an unsafe contract: the caller must
guarantee that the embedding container remains pinned and writable, and is not already hosting
another live event in that field, for as long as the endpoints returned by `Event::placed()`
exist. Endpoints obtained this way are otherwise indistinguishable in behavior from endpoints
obtained through any other construction path.
