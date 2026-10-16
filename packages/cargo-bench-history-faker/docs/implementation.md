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

## Preparation cost and cleanup

Stable paths trade automatic reclamation for retained final artifacts. Each
`_faker-path` invocation creates another `faker/run-*` directory, even when Cargo
finds the build fresh. Recipes that prebuild the faker (`test`, `careful`,
`coverage-measure`, `mutants`, `test-azurite` and `test-azure`) each prepare it once
per invocation, regardless of whether the selected tests need it. Compound recipes
inherit those preparations.

Without an existing `CBH_FAKER`, direct nextest execution prepares one executable
per test process that calls the locator. Repeated calls within that process reuse
its cached path. A valid prebuilt override avoids these preparations. Ordinary
`cargo build` and launching an already prepared faker do not create `run-*`
directories.

### Disk space

Cargo uses hard links for final artifacts where supported: multiple paths refer
to the same underlying file data. Repeated preparations of an unchanged cached
build therefore add directories, links and small metadata files rather than full
executable copies. Rebuilds that replace cached artifacts can leave older file
data retained through the prepared paths. If hard linking is unavailable and
Cargo copies instead, each preparation can retain a full output set.

As a sizing reference, a Windows x64 debug output is approximately 0.7 MiB for the
executable plus 5.4 MiB for its PDB, or 6.1 MiB together. Both files can share
storage through hard links. Sizes depend on the platform, toolchain and build
settings; this is not a fixed storage budget. Intermediate compilation artifacts
remain shared separately.

There is no automatic cap or expiration. A full `cargo clean` for the corresponding
target tree removes the retained outputs; completing `cargo build`, ending a test
process, or running coverage-specific cleanup does not. Do not clean while
consumers still need their paths. The ownership regression uses its own temporary
root and removes that root after its consumers finish.

### Test execution time without a prebuilt faker

Each consuming nextest process without `CBH_FAKER` starts Cargo, checks workspace
and dependency freshness, waits for applicable Cargo locks, and prepares final
artifacts. The shared intermediate cache avoids compiling the dependency graph
for every test, but does not eliminate those per-process costs. The prebuilding
recipes amortize this work across the suite.

A warm-cache reference sample uses this command with and without a prebuilt
`CBH_FAKER`:

```text
cargo nextest run -p cargo-bench-history --all-features --locked --test cbh_integration
```

On a shared Windows x64 host with 32 logical processors, Rust 1.98.1 and nextest
0.9.143 at default concurrency, paired runs of the 167-test sample take:

| Faker preparation | Nextest execution time | Per-test preparations |
| --- | --- | --- |
| Existing `CBH_FAKER` | 18-28 seconds | None |
| On demand | 29-44 seconds | 153 |

The paired overhead is 11-16 seconds. These timings exclude nextest's initial
build phase and use already compiled tests and a warm faker cache. They illustrate
the cost, not a performance guarantee: machine load, test selection and concurrency
affect it, and a cold cache adds compilation time. This compares prebuilt and
on-demand execution of the same implementation, not shared versus separate final
artifact directories; it does not isolate the cost of allocating stable paths.
