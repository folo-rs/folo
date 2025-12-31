# GitHub Workflows Design Rationale

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
  - **machete** — Multi-platform because conditional compilation affects dependency analysis
  - check-release
  - clippy-release
  - build-release
  - careful

Split from the monolithic `just validate-extra-local` into individual jobs, all multi-platform:

- **mutants** — `timeout-minutes: 90`
  - Runs mutation testing (very slow)
  
- **run-examples** — `timeout-minutes: 30`
  - Executes all example binaries
  
- **hack** — `timeout-minutes: 30`
  - Tests all feature combinations with `cargo hack --feature-powerset`

## Design Decisions

1. **Parallelization over sequential execution** — Individual jobs provide:
   - Faster CI feedback (first failure visible immediately)
   - Clear failure identification (specific check names in GitHub status)
   - Better resource utilization across GitHub runners

2. **Platform matrix considerations**:
   - `format-check` is single-platform (Ubuntu) to save resources
   - `docs` and `machete` remain multi-platform due to conditional compilation differences
   - All other checks run on 3 platforms (ubuntu, macos, windows)

3. **Timeouts**:
   - Default timeout for most jobs
   - Explicit timeouts for long-running jobs to prevent runaway processes

4. **Shared infrastructure**:
   - All jobs use the common `.github/actions/setup-environment` action
   - Rust cache uses `shared-key: prerequisites` for cross-job cache sharing
   - `fail-fast: false` ensures all platform checks complete even if one fails

## Maintenance Notes

- When adding new checks, consider platform-specific needs and appropriate timeouts
- Keep inline comments for single-platform jobs to explain the rationale
- Monitor job run times and adjust timeouts if needed
