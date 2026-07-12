# Selective validation with cargo-delta

You can use `cargo-delta` to validate only the packages that are impacted by your changes,
rather than running full workspace validation or manually trying to identify impacted packages.

Mostly this is used in GitHub workflows, as on local PC it tends to be obvious what is impacted.

## How it works

`cargo-delta` analyzes the current branch and compares it against `origin/main` to determine
which packages are directly modified and which are indirectly affected (via dependency chains).
Only those packages are validated.

Certain files are designated as "trip wires" (see `delta.toml`). If any trip wire file is
changed, all packages are validated regardless. The list is deliberately small - only files that
change what "passing" means for *every* crate and that, changed alone, touch no package (so delta
would otherwise validate nothing): the pinned toolchain (`rust-toolchain.toml`), the MSRV/nightly
pins (`constants.env`), the lint configuration (`clippy.toml`) and the formatting configuration
(`unstable-rustfmt.toml`, whose format check is a hard whole-workspace gate).

Workflow definitions (`.github/**`), the build system (`justfile`, `justfiles/**`) and similar
tooling/config files are intentionally *not* trip wires: they do not change any crate's build/test
correctness, a change that breaks the mechanism fails the PR's own jobs, and the rare miss is
caught by the push-to-main backstop. Treating them as trip wires only forced a full-workspace run
on the many PRs that touch them for unrelated reasons.

The workspace-root `Cargo.toml` and `Cargo.lock` are intentionally *not* trip wires: routine
dependency edits touch them constantly but rarely affect every package, so tripping the whole
workspace on each one would defeat delta scoping. Genuinely cross-cutting manifest changes are
still caught by the push-to-main backstop, which always validates the full workspace. (Per-package
`Cargo.toml` files were never trip wires - the pattern only matched the root manifest - and
continue to scope to their own package.)

## Local usage

```bash
# See which packages are affected by your changes.
just delta

# Run standard validation only on affected packages.
just delta-validate

# Run extended validation (mutants, examples, hack, careful) on affected packages.
just delta-validate-extra
```

You can also manually scope any command to specific packages:

```bash
# Validate a single package.
just package=events_once validate-local

# Validate multiple packages (space-separated).
just package="events_once infinity_pool" validate-local
```

## CI behavior

Pull request builds use delta to validate only impacted packages. Push-to-main builds act as a
backstop and always validate the full workspace. If the backstop catches something that the delta
build missed, the `delta.toml` configuration should be updated to prevent recurrence.
