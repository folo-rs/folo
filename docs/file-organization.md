# File and module organization

This chapter covers how source files, modules, and visibility are organized.
Related topics live in their own chapters: declaring dependencies in
[`docs/dependencies.md`](dependencies.md), and conditional compilation / feature
gating in [`docs/feature-flags.md`](feature-flags.md).

## Filesystem structure

We prefer many smaller files over few large files, typically only packing
implementation details and unit tests into the same file but keeping separate
API-visible types in separate files (even if only API-visible inside the same
crate).

We prefer to keep the public API relatively flat — even if we create separate Rust
modules for types, we re-export them all at the parent. So while we have modules
like `packages/many_cpus/src/hardware_tracker.rs`, the type itself is exported at
the crate root as `many_cpus::HardwareTracker` instead of at the module as
`many_cpus::hardware_tracker::HardwareTracker`.

`lib.rs` and `mod.rs` files should only contain API documentation and
re-exports. Do not define constants, helper functions, or other items in these
files — move them to dedicated files (e.g. `constants.rs`) for better filesystem
organization.

Inline `mod` blocks defined directly in `lib.rs` / `mod.rs` count as "re-exports"
for the purpose of this rule as long as they only re-export items from other
modules (no logic, no constants, no new type definitions). This is common for
`#[doc(hidden)] pub mod __private { pub use ...; }` or similar grouping modules
whose sole purpose is to gather and re-expose existing items under a distinct name.

## File contents flow

Bigger and more important types go higher in the file, smaller and less important
types go lower.

Public API types go higher in the file, private types go lower.

The implementation of a type should stay close to the definition of the type (e.g.
`impl` blocks of a type follow the `struct` block).

## Visibility

Types, fields, functions and methods should have the minimum required visibility,
only being visible in the same file by default if not part of the public API
surface.

Use `pub(crate)` only when a type actually needs to be accessible in other files
of the same crate. We do not use `pub(super)` and other fine-grained visibility
modifiers - they are too much hassle to maintain.

## Macros may need public-private visibility

For purpose of accessing members of types or modules from published macros, it is
permissible to make logically private items public. This must be accompanied by
two adjustments:

* The item must be marked with `#[doc(hidden)]` to prevent it from being included
  in the public API documentation. This is our signal that it is not an officially
  supported public API.
* The path to the item must include the keyword "private". The preferred forms
  are:
  * For types, use a `private` module, e.g. `foo::private::Bar`.
  * For methods, use a `__private_` prefix, e.g. `Bar::__private_reset_counters()`.

For exposing private surface to in-workspace benches and external test crates,
prefer the `_impl` crate split pattern instead — see
[`docs/impl-crate-split.md`](impl-crate-split.md).
