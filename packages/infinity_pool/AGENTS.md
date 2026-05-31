# Agent notes for infinity_pool

## Design intent: long-lived pools

`infinity_pool` is designed for **long-lived pools**, not short-lived or
one-shot ones. This drives several deliberate trade-offs that should not be
"optimized away":

* **Slab initialization cost on the first insert is intentional.** Initializing
  all slot metadata up front when a slab is allocated keeps the steady-state
  insert path as simple as possible (no per-op branch to check whether the slot
  has been touched yet). Schemes that defer initialization (`MaybeUninit` +
  high-water mark, piecewise lazy init) save the empty-pool first-insert cost
  at the price of permanently slowing the steady-state path — the wrong
  trade.
* **First-insert / empty-pool latency is not a target.** Pools that are
  constructed, used briefly, and then dropped are not the workload we
  optimize for. Optimizations whose entire value is in the first insert
  (`alloc_zeroed`, lazy init, etc.) are only acceptable if they add no
  meaningful complexity.
* **Per-slab bulk drop is an edge case.** Optimizing `Slab::drop` (e.g. with
  an `any_needs_drop` flag, occupancy bitmap, etc.) is not worth the design
  complexity unless a concrete user-facing scenario motivates it. Pools that
  drop are pools that are being torn down — the cost is amortized over the
  pool's entire lifetime.

## Pooled types typically need `Drop`

Real-world payloads (`String`, complex structs, smart pointers) usually have
non-trivial drop. Optimizations specialized on `!mem::needs_drop::<T>()` for
typed pools deliver minimal practical value because the common case is the
opposite. Do not propose monomorphizing `Slab` on `needs_drop::<T>()` or
similar specializations unless a concrete drop-free workload motivates it.

The surgical complement — making the per-handle dropper invocation a no-op
when `T` does not need drop, with no other type-level changes — has already
landed and represents the right level of effort for this trade-off.

## `SlotMeta` runtime checks are intentional

The `unreachable!()` arm in the slot-meta state machine is intentional
defense-in-depth: a panic there indicates a corrupted state machine, which
in practice means a thread-safety bug. Do not remove it to save a `cmpq`.
If a measured need to remove the check ever arises, use `unreachable_unchecked!()`
as a surgical alternative — do not propose representation changes (`repr(C)`
slot metadata) for this purpose.

## Stay idiomatic — do not "code Rust as if it were C"

Manually controlling the in-memory representation of slab metadata
(`repr(C)` slot structs, `alloc_zeroed`-friendly layouts, explicit
discriminant encoding) is explicitly out of scope. Trust the Rust compiler's
layout decisions. If a niche encoding is being exploited and could in
principle change between rustc releases, that is the language's concern,
not ours.

## What good infinity_pool optimizations look like

[PR #194](https://github.com/folo-rs/folo/pull/194) (`#[inline]` on
`<Dropper as Drop>::drop`) is the canonical example: 5 lines added, no
behavior change, Callgrind delta verified (−6% on
`pinned_pool_drop_handle_from_10k`), and a justifying inline comment per
the workspace `#[inline]` policy. Surgical interventions that change one
small thing, preserve all invariants, and are backed by a disassembly
observation are the kind of optimization that lands.

Optimization proposals that touch slot-metadata representation, monomorphize
`Slab` on type traits, defer slab initialization, or add per-slab tracking
state are unlikely to land without a concrete user-facing scenario that
motivates the design complexity.
