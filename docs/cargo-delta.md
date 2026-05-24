# Selective validation with cargo-delta

When working on a feature branch, you can use `cargo-delta` to validate only the packages that
are impacted by your changes, rather than running full workspace validation.

## How it works

`cargo-delta` analyzes the current branch and compares it against `origin/main` to determine
which packages are directly modified and which are indirectly affected (via dependency chains).
Only those packages are validated.

Certain files are designated as "trip wires" (see `delta.toml`). If any trip wire file is
changed, all packages are validated regardless. Trip wires include workspace-wide configuration
files (`Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, etc.), workflow definitions and the
build system itself.

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
