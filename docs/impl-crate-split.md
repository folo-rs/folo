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

The historical workaround was the `test-util` Cargo feature on the **public** crate:
gate a public-but-internal constructor like `Report::fake` behind
`#[cfg(any(test, feature = "test-util"))]` on `foo`, then add `test-util` to every
dev-dependency that needs it. This works but has real costs:

- The feature flag becomes part of the public API surface of `foo`. Users see it
  on docs.rs and may reasonably depend on it. Removing the flag is a breaking change.
- Documentation conditionally renders items based on enabled features, so the rendered
  API drifts depending on the build configuration.
- Self-dev-dependencies (`foo = { path = ".", features = ["test-util"] }`) are an odd
  workaround required to make integration tests in the same crate see the feature-gated
  items.
- Every public `fake`/`new_for_test` constructor must carry the `#[cfg(...)]` gate,
  the `#[doc(hidden)]` attribute, and a comment explaining its existence. This noise
  spreads through the implementation.

Moving the `test-util` feature down to `foo_impl` (which is `[lib] doc = false`,
`#![doc(hidden)]`, and documented as "do not depend on this directly") sidesteps
the first two costs entirely: the feature is invisible to docs.rs of `foo`, never
on `foo`'s public API surface, and never activated by `foo` itself. The remaining
two costs — self-dev-dep wiring and per-item `#[cfg]` noise — go away because items
can be plain `pub` in `foo_impl` and reached by external benches/tests directly.
See "Test-only fabrication constructors" below for how this applies to the narrow
case of fabrication helpers that still benefit from a feature gate.

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
  outside the crate boundary (no `#[cfg]` gates, no `#[doc(hidden)]` decorators,
  no feature flags). This is the central simplification the split unlocks.
- Items that genuinely only need to be visible within the impl crate stay
  `pub(crate)` as usual.
- The narrow exception to "no feature flags" is **test-only fabrication
  constructors**: see "Test-only fabrication constructors" below.

Concretely, `foo_impl` becomes a "private back door" that is *technically* public but
documented as off-limits via:

- `description = "Implementation crate for `foo` - do not reference directly"` in `Cargo.toml`
- `#![doc(hidden)]` at the crate root
- `[lib] doc = false` in `Cargo.toml` (so `cargo doc` does not render it)
- A `README.md` that says "do not depend on this directly"
- A pointer back to `foo` in the crate-level rustdoc

The fact that `foo_impl` is published is mechanical: Cargo cannot depend on an
unpublished crate from a published one, and we want `foo` to actually publish.

## Versioning: `foo` and `foo_impl` are released in lockstep

`foo` and `foo_impl` are logically the same package. We do not maintain a SemVer
boundary between them, and we do not want downstream consumers to ever end up
with a mismatched pair (e.g. `foo 1.5.0` paired with `foo_impl 1.4.7`). Two
mechanisms enforce lockstep versioning together:

1. **`foo` exact-pins `foo_impl` in its `Cargo.toml`**, using the `=X.Y.Z`
   constraint form:

   ```toml
   # foo/Cargo.toml
   [dependencies]
   # Exact-version pin: foo and foo_impl are logically the same package split
   # into two crates for cargotechnical reasons. release-plz `version_group`
   # keeps them in lockstep at release time; the `=` pin enforces at the Cargo
   # manifest level that downstream consumers never end up with a mismatched
   # pair.
   foo_impl = { version = "=1.5.0", path = "../foo_impl", default-features = false }
   ```

   Note that `foo` declares the `foo_impl` dependency directly (not via
   `workspace = true`). This is a deliberate deviation from the workspace-wide
   convention of inheriting deps from `[workspace.dependencies]`: the
   tight-coupling relationship is exactly what we want the reader of
   `foo/Cargo.toml` to see at a glance, and inheriting via `workspace = true`
   would hide it.

2. **`release-plz.toml` puts both packages in the same `version_group`**, which
   makes release-plz bump them in lockstep on every release cycle:

   ```toml
   # release-plz.toml
   [[package]]
   name = "foo"
   version_group = "foo"

   [[package]]
   name = "foo_impl"
   version_group = "foo"
   ```

   `version_group` is a release-plz feature, not a Cargo feature. It only acts
   at release time. The `=` pin in `foo/Cargo.toml` is the Cargo-level
   complement: even if a future change to `release-plz.toml` accidentally
   dropped the group, the published `foo` would still refuse to install with a
   different `foo_impl` version.

When a release happens, release-plz updates the `=X.Y.Z` constraint in
`foo/Cargo.toml` to point at the new `foo_impl` version automatically — there
is no manual maintenance step. Bumping either package out of lockstep would
require a deliberate manual edit and would be caught at the next release-plz
check.

The same lockstep treatment is **not** appropriate for ordinary "I depend on
crate X" relationships, where a SemVer range gives Cargo room to satisfy
multiple consumers. It is appropriate here precisely because `foo_impl` is not
a real third-party dependency — it is `foo`'s own implementation under a
different crate name.

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

## Where each kind of artifact lives

The split changes which crate owns each kind of artifact. The audience of the
artifact is the deciding factor, not what API surface it happens to need today.

| Artifact                          | Home crate | Rationale |
| --------------------------------- | ---------- | --------- |
| `src/**` (implementation)         | `foo_impl` | The whole point of the split. |
| Unit tests (`#[cfg(test)] mod tests` inside `src/**`) | `foo_impl` | They follow the code they test. |
| `benches/**`                      | `foo_impl` | Benches are a maintainer tool; whoever runs them already knows the impl crate exists. Hosting them in `foo_impl` means a future bench can reach for `test-util` fabrication constructors or other internal `pub` items without having to be relocated first. |
| `tests/**` (integration tests)    | `foo_impl` | Same reasoning as benches: they are for maintainers, and proximity to `foo_impl` internals is occasionally needed. |
| `examples/**` (user-facing)       | `foo`      | Examples are a form of end-user documentation. They must compile against the same public API a user gets from `cargo add foo`. Keeping them in the public crate is what enforces that they cannot accidentally reach for internals. |
| Maintainer-only demo/dev binaries | `foo_impl/examples/` (optional) | If you do want a runnable internal demo (e.g. a load-generator, a profiling harness, a "wire up the internals to see what they do" app), put it under `foo_impl/examples/`. Treat it explicitly as "examples and dev apps for maintainers", a different category from end-user examples. |
| Re-export smoke test              | `foo`      | The one and only `tests/` file in `foo`. It exists specifically to assert that the explicit re-export list in `foo/src/lib.rs` keeps reaching every advertised item. See step 10 below. |

Two principles fall out of the table:

1. **The split is along an audience boundary, not a language-feature boundary.**
   `foo` is for users; `foo_impl` is for maintainers. Examples are user
   documentation, so they stay with the users. Benches and integration tests
   are maintainer tooling, so they stay with the maintainers.
2. **Benches and integration tests living in `foo_impl` should still prefer
   the public API.** Reach for `test-util` fabrication constructors or other
   `foo_impl`-internal items only when there is a specific reason —
   fabricating internal state for a contract that the public API cannot
   express, or measuring a hot path that the public API gates behind a slower
   entry point. When a bench uses only `use foo::Event;` it is still measuring
   what the user pays; the impl crate is just a convenient host.

## Test-only fabrication constructors

Some internal items only make sense in a test or benchmark context: constructors
that fabricate data structures bypassing the real-world entry path (e.g. building
a `Report` from a hand-assembled `Vec<EventMetrics>` without observing real
events). Two competing concerns apply:

1. They must be reachable from external benches/tests in other workspace crates,
   which would otherwise force them to be `pub` in `foo_impl`.
2. They are pure dead weight in production builds and should not bloat the
   binary that real users ship.

The convention is to put them directly on the type they fabricate, gated behind
a `test-util` Cargo feature on `foo_impl`:

```rust
// in foo_impl/src/reports.rs

impl Report {
    /// Constructs a `Report` from pre-assembled parts without touching the
    /// global event registry.
    ///
    /// Intended for in-workspace tests and benchmarks. Gated behind the
    /// `test-util` feature on `foo_impl` so it is never compiled into end-user
    /// builds of `foo`; the `foo` shell crate does not activate that feature.
    #[cfg(any(test, feature = "test-util"))]
    #[doc(hidden)]
    #[must_use]
    pub fn fake(events: Vec<EventMetrics>) -> Self {
        Self {
            events: events.into_boxed_slice(),
        }
    }
}
```

```toml
# foo_impl/Cargo.toml
[features]
test-util = []
```

Why this is safe to call a feature here even though the broader pattern avoids
them:

- `foo_impl` is `[lib] doc = false`, `#![doc(hidden)]`, and documented as
  "do not depend on this directly". The feature flag never appears on docs.rs
  for `foo_impl` because `foo_impl` is not rendered.
- `foo`'s `Cargo.toml` declares `foo_impl = { workspace = true }` without
  activating the feature. End users running `cargo add foo` therefore never
  get `fake` constructors compiled into their build.
- In-workspace consumers (`foo_otel`, etc.) activate the feature in their
  dev-dependencies: `foo_impl = { path = "../foo_impl", features = ["test-util"] }`.
  Cargo's feature unification then makes `Report::fake` available across the
  workspace's test/bench build graph.
- The `cfg(any(test, feature = "test-util"))` form means `foo_impl`'s own
  unit tests (`#[cfg(test)] mod tests`) automatically see the constructors
  without anyone needing to opt the workspace into the feature.

Call sites become natural and symmetric with the rest of the public API:

```rust
use foo::{EventMetrics, Histogram, Report};

let report = Report::fake(vec![
    EventMetrics::fake("event_a", 10, 100, None),
    EventMetrics::fake("event_b", 20, 200, Some(Histogram::fake(...))),
]);
```

Note that the call sites name the types through `foo` (the public crate), not
through `foo_impl`. The `fake` constructors are inherent methods on the type,
so they are reachable via any path that names the type — and `foo`'s re-exports
expose the type, even though they don't advertise the `fake` constructor. The
`foo_impl` dev-dependency is needed only as a Cargo manifest entry to activate
the `test-util` feature; no `use foo_impl::*;` import is required in test code.

### When to skip the feature flag

The feature flag is the recommended default because it keeps fabrication code
out of release binaries. If the fabrication helpers are trivial and the
"out of release builds" purity isn't worth the extra Cargo manifest line for
each consumer, plain `#[doc(hidden)] pub fn fake(...)` (with no `#[cfg]`) is
also acceptable. The impl-crate-split convention is the primary isolation
mechanism in either case; the feature flag is a secondary belt-and-suspenders.

## How to do the split

1. Create `packages/foo_impl/` with a `Cargo.toml` that mirrors `foo`'s former
   dependencies, plus `[lib] doc = false` and `description = "Implementation crate
   for `foo` - do not reference directly"`. Start version at `0.1.0` regardless of
   the version of `foo` (this is a brand-new crate).

2. Add `foo_impl` to the workspace's `[workspace.dependencies]` table.

3. Move every source file under `packages/foo/src/`, plus every `benches/`
   and `tests/` file, into the matching folder under `packages/foo_impl/`.
   Leave `packages/foo/examples/` where it is — examples stay with the
   public crate (see the placement table above). Use `git mv` so history
   follows.

4. Replace `packages/foo/src/lib.rs` with a thin shell:
   - Preserve the entire crate-level `//!` documentation (this is what users will
     read on docs.rs).
   - Add an explicit `pub use foo_impl::{TypeA, TypeB, ...};` listing every item
     that should be part of the public API. **Do not** use a wildcard re-export
     here — the explicit list is the contract.

5. Replace `packages/foo/Cargo.toml` with a thin manifest. Declare `foo_impl`
   as a direct path dependency with an exact-version pin and any dev-deps
   needed for re-export smoke tests:

   ```toml
   [dependencies]
   foo_impl = { version = "=X.Y.Z", path = "../foo_impl", default-features = false }
   ```

   Do **not** inherit `foo_impl` from `[workspace.dependencies]` via
   `workspace = true`. The whole point of declaring it here is to make the
   tight coupling and the exact-version pin visible at the consumption site.
   See "Versioning" above for the rationale.

6. Add `version_group` entries for both packages to `release-plz.toml`:

   ```toml
   [[package]]
   name = "foo"
   version_group = "foo"

   [[package]]
   name = "foo_impl"
   version_group = "foo"
   ```

   This makes release-plz bump both packages to the same version on every
   release cycle, and update the `=X.Y.Z` pin in `foo/Cargo.toml`
   automatically so it always points at the just-released `foo_impl`.

7. Inside `foo_impl/src/lib.rs`:
   - Put `#![doc(hidden)]` at the top.
   - Add a brief crate-level doc comment that points to `foo`.
   - Declare modules and re-export items just as the original `foo` lib.rs did.

8. Convert any `#[cfg(any(test, feature = "test-util"))] pub fn fake(...)`
   constructors that already existed in `foo` so they live in `foo_impl` instead.
   Add a `test-util = []` feature to `foo_impl/Cargo.toml`. Keep the
   `cfg(any(test, feature = "test-util"))` gate, the `#[doc(hidden)]` attribute,
   and the `pub fn fake(...)` signature directly on the type. See "Test-only
   fabrication constructors" above for the full rationale.

9. Update consumers (in-workspace dev-dependencies that were using `test-util`
   on `foo`) to declare `foo_impl = { path = "../foo_impl", features = ["test-util"] }`
   as a dev-dependency. Call sites continue to name types through `foo`
   (e.g. `use foo::Report; Report::fake(...)`); no `use foo_impl::*;` import
   is needed because `fake` is an inherent method on the type that `foo`
   re-exports.

10. Add `foo` itself as a dev-dependency of `foo_impl` so doctests written
    from the user's perspective (`use foo::Event;`) compile in `cargo test --doc
    -p foo_impl`. This is an allowed dev-dependency-only cycle.

11. Add a small re-export smoke test in `packages/foo/tests/foo_reexports.rs`
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

## Canonical examples

The pattern has been applied to two crate pairs in this workspace.

### `nm` / `nm_impl`

The original worked example. Concrete files to study:

- `packages/nm/Cargo.toml` — thin shell manifest with the `=0.1.0` exact-version
  pin on `nm_impl` declared as a direct path dependency (not via
  `workspace = true`)
- `packages/nm/src/lib.rs` — preserved crate docs + explicit re-export list
- `packages/nm/tests/nm_reexports.rs` — re-export smoke test
- `packages/nm_impl/Cargo.toml` — impl manifest with `[lib] doc = false`, the
  `test-util` feature for fabrication constructors, and the `nm` dev-dep for
  the doctest cycle
- `packages/nm_impl/src/lib.rs` — `#![doc(hidden)]` root
- `packages/nm_impl/src/reports.rs` — `#[cfg(any(test, feature = "test-util"))]
  #[doc(hidden)] pub fn fake(...)` constructors on `Report`, `EventMetrics`,
  and `Histogram`
- `packages/nm_impl/README.md` — "do not depend on this directly" notice
- `release-plz.toml` — `version_group = "nm"` entries for `nm` and `nm_impl`
  that keep the two packages bumped in lockstep on every release

### `nm_otel` / `nm_otel_impl`

The second worked example. Notable for showing what the pattern looks like
without any `test-util` feature on the impl crate: the internal items that
benches and tests need (`EventState`, `EventState::histogram_deltas`,
`Publisher::run_one_iteration_with_report`) are plain `pub` in `nm_otel_impl`
because they are either real internal collaborators of the production code
path (not fabrication helpers) or trivial wrappers where the
"keep fabrication out of release binaries" purity does not justify the gate.
See "When to skip the feature flag" above.

Concrete files to study:

- `packages/nm_otel/Cargo.toml` — thin shell manifest with the `=0.1.0`
  exact-version pin on `nm_otel_impl` declared as a direct path dependency.
  Activates `nm_impl`'s `test-util` feature only via dev-deps that exercise
  the re-export smoke test; the `nm_otel` library itself does not.
- `packages/nm_otel/src/lib.rs` — preserved user-facing crate docs +
  explicit `pub use nm_otel_impl::{Publisher, PublisherBuilder};`. No
  `__private` module.
- `packages/nm_otel/tests/nm_otel_reexports.rs` — re-export smoke test
- `packages/nm_otel_impl/Cargo.toml` — impl manifest with `[lib] doc = false`,
  the `nm_otel` dev-dep for the doctest cycle, and the `nm_impl` dev-dep with
  `features = ["test-util"]` so the impl-hosted benches can fabricate input
  reports via `Report::fake`.
- `packages/nm_otel_impl/src/lib.rs` — `#![doc(hidden)]` root that
  re-exports the public-API subset for the shell crate plus `EventState`
  for the alloc-tracking test
- `packages/nm_otel_impl/README.md` — "do not depend on this directly" notice
- `release-plz.toml` — `version_group = "nm_otel"` entries for `nm_otel`
  and `nm_otel_impl` that keep the two packages bumped in lockstep on
  every release
