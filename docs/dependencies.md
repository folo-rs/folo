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

