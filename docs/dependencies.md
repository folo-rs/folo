# Dependency management

This chapter covers how dependencies are declared and managed in the `Cargo.toml`
manifests across the workspace.

## Ordering

Dependencies are sorted alphabetically by name of the package.

## Default features

Do not define default features in `Cargo.toml` unless there is a specific,
justified reason to do so. Features should be opt-in, not opt-out.

## Dev-dependencies within workspace packages must be path dependencies

Do not use `version = "1.2.3"` or `workspace = true` when adding a package from
the same workspace as a dev-dependency. Within the same workspace, dev-dependencies
must always be `path = "../foo"` style path-references.

## Lockfile

The committed `Cargo.lock` should track the latest compatible versions of all
dependencies. Refreshing it with `cargo generate-lockfile` is encouraged, and the
incidental transitive-dependency updates this pulls in are welcome — keep them
rather than reverting to a narrower set of changes.

## Version groups and exact-pin cross-references

Some packages are logically one package split into multiple crates for
cargotechnical reasons (see [impl-crate-split.md](impl-crate-split.md)). Examples:
`linked` + `linked_macros` + `linked_macros_impl`, `many_cpus` + `many_cpus_impl`,
`nm` + `nm_impl`, `nm_otel` + `nm_otel_impl`, and `cargo-bench-history` +
its `cbh_*` implementation crates.

Such crates must always be released at the same version. Enforce this in two
places, and keep both in sync:

* **`release-plz.toml`** — give every crate in the set the same `version_group`
  so release-plz bumps them together.
* **The workspace `Cargo.toml`** — the public crate keeps a normal compatible
  version requirement, but every *internal* crate it depends on (the `_impl` /
  `_core` / macro crates) is referenced with an **exact `=` pin**
  (`version = "=1.2.3"`). The exact pin means a downstream consumer of the public
  crate can never resolve a mismatched version of its internal companion.

For example, `many_cpus` is referenced as `version = "2.4.9"` while
`many_cpus_impl` is referenced as `version = "=2.4.9"`; likewise each `cbh_*`
implementation crate is referenced with an exact `=` pin because
`cargo-bench-history` depends on it.

