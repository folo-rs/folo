# GitHub Workflows Design Rationale

Update this ".github/workflows/AGENT.md" file if you change the GitHub workflows.

## Toolchain versions

Rust toolchain versions are defined in `constants.env` (loaded via just's dotenv support) and
`rust-toolchain.toml`. All `just` commands reference these variables instead of hardcoding
version numbers. The GitHub workflows call `just install-tools` and `just <command>`, so
toolchain versions flow automatically from `constants.env` without any duplication in workflows.

## Overview

The CI workflows in this repository run individual `just` commands as separate parallel jobs instead of combined `validate-local` and `validate-extra-local` commands. This design provides faster feedback by parallelizing checks and clearer failure identification.

## Workflow Structure

### validation.yml

Split from the monolithic `just validate-local` into individual jobs:

- **format-check** — Runs only on `ubuntu-latest` (single platform)
  - Rationale: Code formatting rules are platform-agnostic
  - Commands: `cargo +nightly fmt --check`, `cargo sort-derives --check`

- **Multi-platform jobs** (run on ubuntu-latest, macos-latest, windows-latest):
  - check-dev
  - clippy-dev
  - test-more
  - test-docs
  - **docs** — Multi-platform because conditional compilation affects generated documentation
  - miri
  - **test-more-arm** — ARM64 coverage (ubuntu-24.04-arm, windows-11-arm)
    - Exercises ARM-specific code paths (anything gated behind
      `cfg(target_arch = "aarch64")` or similar) which x86_64 runners never compile
      or execute. Platform-neutral code is already validated by the x86_64 matrix.
  - **miri-arm** — ARM64 coverage for Miri (ubuntu-24.04-arm, windows-11-arm)
    - Miri is an interpreter with its own memory model and is architecture-agnostic
      for platform-neutral code. We run it on ARM purely to subject ARM-gated code
      paths to Miri's UB detection — there is no value in running it on ARM for
      code that already compiles on x86_64.
  - **miri-harder-events-once** / **miri-harder-infinity-pool** / **miri-harder-events** — Windows-only, sharded
    - Runs Miri with 64 seeds per test (`-Zmiri-many-seeds=..64`) for select packages
    - Sharded across parallel runners to reduce wall-clock time (4 shards for events_once,
      8 shards for infinity_pool, 2 shards for events)
    - Very slow, so only run for specific packages on a single platform
  - **machete** — Multi-platform because conditional compilation affects dependency analysis
  - check-release
  - clippy-release
  - build-release
  - careful

Split from the monolithic `just validate-extra-local` into individual jobs, all multi-platform:

- **mutants** — `timeout-minutes: 90`
  - Runs mutation testing (very slow)
  
- **run-examples** — `timeout-minutes: 90`
  - Executes all example binaries
  
- **hack** — `timeout-minutes: 90`
  - Tests all feature combinations with `cargo hack --feature-powerset`

### cache-warmup.yml

A scheduled workflow that keeps GitHub Actions caches warm. GitHub evicts caches after
7 days of inactivity, and a cold cache means every parallel validation job must independently
compile all Rust dependencies from scratch (the setup-environment step becomes very expensive).
This workflow runs once daily on all five runner images (ubuntu-latest, windows-latest,
macos-latest, ubuntu-24.04-arm, windows-11-arm) to ensure the
`shared-key: prerequisites` Rust cache is always populated for both x86_64 and ARM64. It also supports `workflow_dispatch`
for manual cache warming after toolchain updates.

## Design Decisions

1. **Concurrency control** — Workflows triggered by push or pull request use the `concurrency`
   key with `group: ${{ github.workflow }}-${{ github.head_ref || github.ref }}` and
   `cancel-in-progress: true`. This automatically cancels in-progress runs when new commits
   are pushed to the same PR branch or to main, avoiding wasted runner time on outdated code.
   The `cache-warmup` workflow is excluded because it is schedule-triggered and not
   commit-driven.

2. **Parallelization over sequential execution** — Individual jobs provide:
   - Faster CI feedback (first failure visible immediately)
   - Clear failure identification (specific check names in GitHub status)
   - Better resource utilization across GitHub runners

   Expensive jobs are gated behind cheaper equivalents so a fast failure short-circuits
   the expensive work and avoids burning runner time on code that already failed a
   simpler check:
   - `check-release` depends on `check-dev`
   - `clippy-release` depends on `clippy-dev`
   - `mutants` depends on `test-more` (mutation testing is meaningless if base tests fail)
   - `miri-harder-*` depend on both `miri` and `miri-arm` (many-seeds runs are
     orders of magnitude slower than a single Miri pass)

3. **Platform matrix considerations**:
   - `format-check` is single-platform (Ubuntu) to save resources
   - `docs` and `machete` remain multi-platform due to conditional compilation differences
   - `miri-harder-*` jobs are Windows-only due to high cost; sharded across parallel runners
   - Most checks run on the 3 x86_64 platforms (ubuntu, macos, windows)
   - `test-more-arm` and `miri-arm` extend coverage to ARM64 (ubuntu-24.04-arm,
     windows-11-arm) solely to exercise ARM-gated code paths
     (`cfg(target_arch = "aarch64")` and similar). Platform-neutral code is already
     covered by the x86_64 matrices — Miri in particular is an interpreter with its
     own memory model and is architecture-agnostic for platform-neutral code.

4. **Timeouts**:
   - Default timeout for most jobs
   - Explicit timeouts for long-running jobs to prevent runaway processes

5. **Shared infrastructure**:
   - All jobs use the common `.github/actions/setup-environment` action
   - Rust cache uses `shared-key: prerequisites` for cross-job cache sharing
   - Cache key includes the runner image version via `env-vars: ImageVersion`
     so caches do not flow across runner image releases (cached binaries can
     subtly depend on image-specific dylibs and system tools, even when the
     lockfile and rust toolchain are identical)
   - `fail-fast: false` ensures all platform checks complete even if one fails

## Maintenance Notes

- When adding new checks, consider platform-specific needs and appropriate timeouts
- Keep inline comments for single-platform jobs to explain the rationale
- Monitor job run times and adjust timeouts if needed

## System dependencies

The `setup-environment` action installs Valgrind on Linux runners and `gungraun-runner` (via
`just install-tools`). Both are required for the Callgrind `*_cg` bench targets that
live in `packages/*/benches/`. Even when those targets are not executed, `cargo test
--benches` (which `test-more` uses) invokes each bench binary's `main()` once, which forwards
its args to `gungraun-runner`. Without `gungraun-runner` on `$PATH` the call fails with
`Failed to run benchmarks: No such file or directory`, which breaks `test-more` and
`coverage`. The Callgrind bench binaries are compiled to no-op stubs on Windows and macOS via
`#[cfg(not(target_os = "linux"))] fn main() {}`, so neither tool is required there.
