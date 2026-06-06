# The `_impl` crate split pattern

This document describes a structural convention we use to expose private API surface
to benchmarks and tests **outside the crate boundary** without leaking it to public
consumers. The pattern is: factor the implementation of a public crate `foo` into a
companion crate `foo_impl`, and make `foo` a thin re-export shell over `foo_impl`.

## Motivation

Rust's `pub`/`pub(crate)` visibility model has one major friction point for crates
that ship benchmarks or integration tests as separate compilation units:

- A benchmark in `packages/foo/benches/foo_internals.rs` is its own compilation unit
  outside the `foo` crate. It can only call into `pub` items.
- An integration test in `packages/foo/tests/` is also outside the crate.
- An external bench/test in a *different* package (e.g. `foo_otel` benchmarking the
  export path with fabricated reports) cannot reach `pub(crate)` items either.

The historical workaround was the `test-util` Cargo feature: gate a public-but-internal
constructor like `Report::fake` behind `#[cfg(any(test, feature = "test-util"))]`, then
add `test-util` to every dev-dependency that needs it. This works but has real costs:

- The feature flag becomes part of the public API surface. Users see it on docs.rs
  and may reasonably depend on it. Removing the flag is a breaking change.
- Documentation conditionally renders items based on enabled features, so the rendered
  API drifts depending on the build configuration.
- Self-dev-dependencies (`foo = { path = ".", features = ["test-util"] }`) are an odd
  workaround required to make integration tests in the same crate see the feature-gated
  items.
- Every public `fake`/`new_for_test` constructor must carry the `#[cfg(...)]` gate,
  the `#[doc(hidden)]` attribute, and a comment explaining its existence. This noise
  spreads through the implementation.

## The split

For a published crate `foo`, the pattern is:

| Crate      | Role                                                                  |
| ---------- | --------------------------------------------------------------------- |
| `foo`      | Thin shell. All user-facing documentation lives here. Re-exports the public-API subset of items from `foo_impl`. |
| `foo_impl` | Hosts the entire implementation. `#![doc(hidden)]` crate root. Published only so `foo` can depend on it. Marked `[lib] doc = false`. |

The visibility model becomes:

- Items that are `pub` in `foo_impl` are reachable by anything that depends on
  `foo_impl` (such as `foo` itself, plus any in-workspace dev-dependency).
- The `foo` crate re-exports a *narrow, explicit list* of items via `pub use foo_impl::{...}`
  (no wildcards). Anything not on that list is not part of `foo`'s public API even
  though it is `pub` in `foo_impl`.
- The implementation can use plain `pub` for things benches/tests need to reach
  (no `#[cfg]` gates, no `#[doc(hidden)]` decorators, no feature flags).
- Items that genuinely only need to be visible within the impl crate stay
  `pub(crate)` as usual.

Concretely, `foo_impl` becomes a "private back door" that is *technically* public but
documented as off-limits via:

- `description = "Implementation crate for `foo` - do not reference directly"` in `Cargo.toml`
- `#![doc(hidden)]` at the crate root
- `[lib] doc = false` in `Cargo.toml` (so `cargo doc` does not render it)
- A `README.md` that says "do not depend on this directly"
- A pointer back to `foo` in the crate-level rustdoc

The fact that `foo_impl` is published is mechanical: Cargo cannot depend on an
unpublished crate from a published one, and we want `foo` to actually publish.

## When to apply this pattern

Apply this split when at least one of these holds:

- The crate has (or wants) benchmarks or external tests that need access to internal
  types, constructors, or invariants that should not be part of the public API.
- The crate currently uses a `test-util` Cargo feature (or equivalent) purely to
  expose private types for in-workspace consumers.
- Another crate in the workspace needs to construct internal data structures for
  testing or benchmarking purposes.

Do **not** apply this split when:

- The "internal" API is actually a legitimate testing API for external users
  (e.g. `many_cpus::SystemHardware::fake()` is a documented testing aid that
  downstream crates use to write tests against `many_cpus`). Such APIs belong
  in the public crate, fully documented.
- The crate has no benchmarks or external tests at all.
- All internal items only need to be reached by the crate's own unit tests
  (`#[cfg(test)] mod tests` inside the same file already sees `pub(crate)` items).

## The `TestFacade` pattern

When the impl crate needs to expose constructors specifically for test/bench
fabrication (replacing the old `Report::fake`, `EventMetrics::fake`, `Histogram::fake`
constructors), gather them on a single `TestFacade` type:

```rust
// in foo_impl/src/test_facade.rs

#[derive(Debug)]
#[non_exhaustive]
pub struct TestFacade;

impl TestFacade {
    #[must_use]
    pub fn report(events: Vec<EventMetrics>) -> Report {
        Report::from_parts(events)
    }

    #[must_use]
    pub fn event_metrics(/* ... */) -> EventMetrics { /* ... */ }

    #[must_use]
    pub fn histogram(/* ... */) -> Histogram { /* ... */ }
}
```

Conventions for `TestFacade`:

- Lives in `foo_impl::test_facade` and is re-exported at the impl-crate root.
- **Not** re-exported from `foo`. The whole point is that it stays out of the public API.
- Use `#[non_exhaustive]` so adding a new associated function is not a breaking change.
- Real "from parts" constructors stay `pub(crate)` on their owning types; `TestFacade`
  is the only `pub` entry point. This keeps the type's own public API surface clean
  and confines fabrication to one well-known location.

Call sites become:

```rust
use foo_impl::TestFacade;

let report = TestFacade::report(vec![TestFacade::event_metrics(...)]);
```

The `foo_impl` dependency is declared as a dev-dependency on the consumer crate.

## How to do the split

1. Create `packages/foo_impl/` with a `Cargo.toml` that mirrors `foo`'s former
   dependencies, plus `[lib] doc = false` and `description = "Implementation crate
   for `foo` - do not reference directly"`. Start version at `0.1.0` regardless of
   the version of `foo` (this is a brand-new crate).

2. Add `foo_impl` to the workspace's `[workspace.dependencies]` table.

3. Move every source file under `packages/foo/src/`, plus every `benches/` and
   `tests/` file, into the matching folder under `packages/foo_impl/`. Use `git mv`
   so history follows.

4. Replace `packages/foo/src/lib.rs` with a thin shell:
   - Preserve the entire crate-level `//!` documentation (this is what users will
     read on docs.rs).
   - Add an explicit `pub use foo_impl::{TypeA, TypeB, ...};` listing every item
     that should be part of the public API. **Do not** use a wildcard re-export
     here — the explicit list is the contract.

5. Replace `packages/foo/Cargo.toml` with a thin manifest that depends only on
   `foo_impl = { workspace = true }` and any dev-dependencies needed for re-export
   smoke tests.

6. Inside `foo_impl/src/lib.rs`:
   - Put `#![doc(hidden)]` at the top.
   - Add a brief crate-level doc comment that points to `foo`.
   - Declare modules and re-export items just as the original `foo` lib.rs did.

7. Convert any `#[cfg(any(test, feature = "test-util"))] pub fn fake(...)`
   constructors to `pub(crate) fn from_parts(...)` constructors. Add a
   `TestFacade` type (see above) that wraps them with `pub` associated functions.

8. Update consumers (in-workspace dev-dependencies that were using `test-util`)
   to declare `foo_impl = { path = "../foo_impl" }` as a dev-dependency and to
   call `TestFacade::*` instead of `Type::fake(...)`.

9. Add `foo` itself as a dev-dependency of `foo_impl` so doctests written
   from the user's perspective (`use foo::Event;`) compile in `cargo test --doc
   -p foo_impl`. This is an allowed dev-dependency-only cycle.

10. Add a small re-export smoke test in `packages/foo/tests/foo_reexports.rs`
    that pulls every re-exported item by name and exercises just enough of it
    to confirm the contract. This catches regressions if a re-export is
    accidentally dropped from the list in step 4.

## Distinction from the `__private` module convention

This workspace already has a convention for exposing items to **proc macros** within
the same crate: place them under `#[doc(hidden)] pub mod __private { ... }` or
prefix methods with `__private_`. That convention solves a different problem —
macro-expanded code needs to reach items in the crate that defined the macro —
and the items live in the **same** crate, not a separate one.

The `_impl` crate split is a complementary mechanism that crosses the crate
boundary. The two patterns can coexist within the same project:

- Use `__private` / `__private_` for items that need to be visible to a proc-macro
  in the same crate.
- Use the `_impl` split for items that need to be visible to **external**
  benchmarks, tests, or other in-workspace crates.

## Canonical example

`packages/nm` / `packages/nm_impl` is the canonical example. Concrete files to study:

- `packages/nm/Cargo.toml` — thin shell manifest
- `packages/nm/src/lib.rs` — preserved crate docs + explicit re-export list
- `packages/nm/tests/nm_reexports.rs` — re-export smoke test
- `packages/nm_impl/Cargo.toml` — impl manifest with `[lib] doc = false` and
  the `nm` dev-dep for the doctest cycle
- `packages/nm_impl/src/lib.rs` — `#![doc(hidden)]` root
- `packages/nm_impl/src/test_facade.rs` — `TestFacade` pattern
- `packages/nm_impl/README.md` — "do not depend on this directly" notice
