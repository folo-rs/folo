# Faker implementation

The executable parses arguments and delegates engine output to the library's
writers. Each writer separates a pure document-producing core from filesystem
output. Unit tests exercise value transformations; integration tests exercise
filesystem and process behavior.

## Executable ownership

The locator and `just _faker-path` allocate a new final artifact directory for
each preparation under the active target tree's `faker` directory. Cargo's
`CARGO_BUILD_BUILD_DIR` points to the shared `faker/build` cache, while
`--target-dir` points to the preparation's unique output directory. This uses
stable Cargo support available at the workspace MSRV.

Cargo manages intermediate-cache locking. Readers never receive a path
within that cache or within another preparation's artifact directory, so a later
build cannot unlink their executable. The final output directories remain until
explicit target-tree cleanup with `cargo clean`: a prebuild process exits before its consumers run,
and a process-local temporary-directory guard would have the wrong lifetime.

The locator's explicit-input preparation entry point also lets integration tests
own an isolated cache. They launch an already prepared executable while preparing
further executables concurrently, then remove the cache deterministically and
launch every retained executable again. This checks
file ownership and real process execution without depending on a particular
scheduler interleaving. JSON artifact parsing and target-path mapping remain pure
unit tests.

The preparation recipe is a thin PowerShell process boundary: it selects the
pinned toolchain, allocates an output name, sets Cargo's cache path and forwards
the executable reported by Cargo. The locator implements the same output/cache
separation for direct Cargo test invocations without recipe preparation.
