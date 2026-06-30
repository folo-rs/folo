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
  - test-more-x64
  - test-docs
  - **docs** — Multi-platform because conditional compilation affects generated documentation
  - miri-x64
  - **test-more-arm** — ARM64 coverage (ubuntu-24.04-arm, windows-11-arm)
    - Exercises ARM-specific code paths (anything gated behind
      `cfg(target_arch = "aarch64")` or similar) which x86_64 runners never compile
      or execute. Platform-neutral code is already validated by the x86_64 matrix.
    - Paired with `test-more-x64`; both carry an explicit `-x64`/`-arm` suffix for
      symmetry. Other jobs that have no ARM counterpart remain unsuffixed.
  - **miri-arm** — ARM64 coverage for Miri (ubuntu-24.04-arm, windows-11-arm)
    - Miri is an interpreter with its own memory model and is architecture-agnostic
      for platform-neutral code. We run it on ARM purely to subject ARM-gated code
      paths to Miri's UB detection — there is no value in running it on ARM for
      code that already compiles on x86_64.
    - Paired with `miri-x64`; both carry an explicit `-x64`/`-arm` suffix for symmetry.
  - **miri-harder-events-once** / **miri-harder-infinity-pool** / **miri-harder-events** — Windows-only, sharded
    - Runs Miri with 64 seeds per test (`-Zmiri-many-seeds=..64`) for select packages
    - Sharded across parallel runners to reduce wall-clock time (4 shards for events_once,
      8 shards for infinity_pool, 2 shards for events)
    - Very slow, so only run for specific packages on a single platform
  - **machete** — Multi-platform because conditional compilation affects dependency analysis
  - check-release
  - clippy-release
  - build-release
  - **check-frozen** — verifies the workspace compiles at the minimum dependency versions
    declared in `Cargo.toml` (our published minimum-version promises) rather than the
    higher versions pinned in `Cargo.lock`. Runs the `check-frozen` just recipe, which
    uses the `cargo-freeze-deps` subcommand to rewrite every workspace dependency
    requirement to its declared minimum (`=X.Y.Z`), regenerates the lockfile so the
    resolver selects those minimums, and then runs `check`. A failure means a declared
    minimum is too low to actually compile. Multi-platform because target-specific
    dependencies and conditional compilation can make minimum-version resolution differ
    per platform.
  - careful

Split from the monolithic `just validate-extra-local` into individual jobs, all multi-platform:

- **mutants** — `timeout-minutes: 90`, sharded
  - Runs mutation testing (very slow)
- Sharded 16 ways per platform via cargo-mutants' native `--shard N/M` support; the
    just recipe accepts the same 1-based `N/M` format as `miri-harder` and translates
    to cargo-mutants' 0-based form internally. Without a SHARD argument the recipe
    runs every mutant in a single job, preserving the local-development experience.
  
- **run-examples** — `timeout-minutes: 90`
  - Executes all example binaries
  
- **hack** — `timeout-minutes: 90`
  - Tests all feature combinations with `cargo hack --feature-powerset`

- **test-azurite** — single-platform (Linux) by choice, package-gated to `cargo-bench-history`
  - Runs the Azure storage backend tests against a live Azurite blob emulator and also
    collects coverage so the `azure.rs` network paths reach Codecov.
  - Named for the Azurite emulator it targets; its sibling `test-azure` runs the
    same backend against a real Azure account. Single-platform by choice, not
    limitation: the Azure Blob backend is OS-agnostic network I/O, so one platform
    fully covers it and Linux is the cheapest runner.
  - The Azure backend's network paths **self-skip** when
    no emulator is reachable, so the multi-platform test jobs
    (`test-more`, `coverage`) stay green without one. They run for real only in
    this job: Azurite is provisioned on the runner host (via the `start-azurite`
    composite action) and `BENCH_HISTORY_REQUIRE_AZURITE=1` turns an unreachable
    emulator into a hard failure so it can never silently skip every test.
  - The multi-platform `coverage` job runs without an emulator and therefore
    cannot cover `azure.rs`; this job uploads an `azure`-flagged lcov report that
    Codecov merges with it (a line covered in any upload counts as covered).
  - Linux-only because Azurite runs directly on the host: GitHub service
    containers cannot reliably bind the emulator to a reachable address (the
    default image binds `127.0.0.1` inside the container and the service syntax
    cannot override the command), so the host-process approach is used instead.

- **test-azure** — single-platform (Linux) by choice, package-gated to `cargo-bench-history`
  - Additive sibling of `test-azurite`: runs the same Azure storage backend
    tests against a **real Azure Storage account** to exercise the **Microsoft
    Entra ID** authentication path that the emulator (account-key/SAS) never
    touches — a real account with shared-key access disabled, reached over HTTPS.
  - Authenticates with **GitHub OIDC workload identity federation** (no stored
    secret): `azure/login@v2` signs in a user-assigned managed identity, leaving
    the Azure CLI authenticated, which the tests' `DeveloperToolsCredential` picks
    up unchanged. Requires `permissions: { id-token: write, contents: read }`.
  - **Double-gated** so it only runs when it can succeed: package-gated to
    `cargo-bench-history` (same gate as `test-azurite`); and **same-repo only** (a
    fork PR cannot mint an OIDC token for our tenant, so the `if:` lets fork PRs skip
    cleanly instead of failing red). The `just test-azure` recipe it invokes sets
    `ENABLE_AZURE=1`, which both opts the real-Azure tests in and turns a missing
    account into a hard failure, so a job that does run can never silently skip every
    test.
  - Reads its Azure identifiers from the repository-root `constants.env` (the same
    non-secret `BENCH_HISTORY_TEST_AZURE_ACCOUNT`, `AZURE_TEST_CLIENT_ID`, `AZURE_TENANT_ID`,
    `AZURE_SUBSCRIPTION_ID` that `just test-azure` reads locally, so local and CI
    target the same account). A `bash` step `grep`s those keys into `$GITHUB_ENV` so
    the `azure/login` inputs and the cleanup step can reference them. It then runs the
    tests via the `just test-azure` recipe (the same one developers use locally; it
    reads the account from the job's `BENCH_HISTORY_TEST_AZURE_ACCOUNT` env and runs the
    `*_in_real_azure` tests). Each test deletes its own container, even on panic; a
    final `if: always()` step runs `infra/azure-bench-history-test/cleanup-containers.ps1`
    as a backstop for a container a crashed run might leave. Because `test-azure` and
    `test-azure-gh` run concurrently against this one account, that backstop passes
    `-MinAgeMinutes 180` so it only sweeps older, genuinely leaked containers and never
    deletes a container the sibling job is still writing to.
  - Collects **no coverage** (`test-azurite` already covers `azure.rs`); its value
    is proving the real Entra + real Blob endpoint round-trip end to end. The
    account, identity and federated credentials are scripted/Bicep'd in
    `infra/azure-bench-history-test/` (see its README to deploy or re-create).

- **test-azure-gh** — single-platform (Linux) by choice, package-gated to `cargo-bench-history`
  - Variant of `test-azure` that exercises cargo-bench-history's **self-minting GitHub
    OIDC credential** — the credential the nightly prod collection (`bench-history.yml`)
    depends on — against the real test account. The `storage/github_oidc.rs` unit tests
    stub the HTTP exchange and `test-azure` takes the local-`az`
    `DeveloperToolsCredential` fallback, so without this job nothing proves the
    self-minting path actually round-trips real GitHub OIDC → Entra → Blob until the
    post-merge nightly run.
  - **Only difference from `test-azure`**: an extra step exports `AZURE_CLIENT_ID`
    (= `AZURE_TEST_CLIENT_ID`). Setting `AZURE_CLIENT_ID` is the switch that makes
    `from_config` build a `ClientAssertionCredential` (which GETs a fresh OIDC JWT,
    audience `api://AzureADTokenExchange`, per Entra token exchange) instead of the
    `DeveloperToolsCredential` it uses otherwise. `permissions: { id-token: write }`
    injects the `ACTIONS_ID_TOKEN_REQUEST_URL`/`TOKEN` values the tool reads to mint
    those assertions, and the test identity's `pull_request` federated credential lets
    same-repo PR runs federate in.
  - `azure/login@v2` is **retained only for harness cleanup**: the per-test
    `delete_container` and the `if: always()` backstop both shell out to `az`, so they
    need a session. The code under test ignores the `az` session entirely once
    `AZURE_CLIENT_ID` is set, so the login does not undermine what the job proves.
  - **Same double-gating** as `test-azure` (package-gated + same-repo only); collects no
    coverage. Together the two jobs cover both credential branches: `test-azure` the
    local-`az` fallback, `test-azure-gh` the CI self-minting path.

### cache-warmup.yml

A scheduled workflow that keeps GitHub Actions caches warm. GitHub evicts caches after
7 days of inactivity, and a cold cache means every parallel validation job must independently
compile all Rust dependencies from scratch (the setup-environment step becomes very expensive).
This workflow runs once daily on all five runner images (ubuntu-latest, windows-latest,
macos-latest, ubuntu-24.04-arm, windows-11-arm) to ensure the
`shared-key: prerequisites` Rust cache is always populated for both x86_64 and ARM64. It also supports `workflow_dispatch`
for manual cache warming after toolchain updates.

### bench-history.yml

A scheduled workflow that collects the workspace's benchmark results into the
long-lived Azure history store every night, building the performance history that
`cargo-bench-history analyze` reads to detect regressions. Only nightly collection
exists today; PR-time collection/validation may follow once this proves out.

- **Multi-platform matrix** (ubuntu-latest, windows-latest, ubuntu-24.04-arm,
  windows-11-arm) with `fail-fast: false`: each OS/architecture is a distinct
  measurement target, and a failure on one platform must not abandon the others'
  history for that night. macOS is omitted — there is no macOS-hosted history store
  consumer yet; add it to the matrix if/when macOS performance tracking is wanted.
- **Whole workspace except the `benchmarks` package**, via the
  `just gh-collect-bench-history` recipe (`cargo-bench-history run --workspace --exclude
  benchmarks --overwrite`). The `benchmarks` package holds slow, special-purpose
  benchmarks that are not part of the tracked history. `--overwrite` makes a re-run on
  an unchanged `main` commit idempotent rather than failing as a duplicate.
- **Uses a dedicated prod managed identity** (`id-folo-bench-history-prod`, provisioned
  by `infra/azure-bench-history-prod/` alongside the account), not the test identity: a
  scheduled run on `main` produces the OIDC subject
  `repo:folo-rs/folo:ref:refs/heads/main`, which matches that identity's `main`-branch
  federated credential. The prod stack is self-contained so the data store never depends
  on test infrastructure. So this workflow needs only `permissions: { id-token: write,
  contents: read }`, an `azure/login@v2` step, and the `AZURE_PROD_CLIENT_ID` /
  `AZURE_TENANT_ID` / `AZURE_SUBSCRIPTION_ID` from `constants.env` (a `bash` step
  `grep`s them into `$GITHUB_ENV`).
- **Writes to a SEPARATE storage account** from the test jobs — the real history store
  `folohistory` (provisioned by `infra/azure-bench-history-prod/`), distinct from the
  throwaway `BENCH_HISTORY_TEST_AZURE_ACCOUNT` the `test-azure`/`test-azurite` jobs
  target. The prod account is baked into the committed `.cargo/bench_history.toml` (where
  cargo-bench-history config belongs), so no account name is surfaced into `$GITHUB_ENV`;
  only the `azure/login` inputs need the grep step.
- **Same-repo gate** (`if: github.repository == 'folo-rs/folo'`): scheduled workflows
  also trigger on forks that enable Actions, but only this repository's identity can
  federate into Azure, so the job skips everywhere else.
- **Auto-detected machine key**: the wall-clock engines partition by the runner's
  auto-detected machine fingerprint (no override) — the `target-triple` already separates
  OS/arch, and an explicit key would risk merging dissimilar hosts under one partition.
- `timeout-minutes: 360`; schedule offset to 03:00 UTC so it does not contend with the
  00:00 `cache-warmup` cron (and runs against an already-warm Rust cache). Like
  `cache-warmup`, it is schedule-triggered and so carries no `concurrency` cancel key.
- **`analyze` job** (`needs: collect`, `if: always()` + same main-only gate) — after
  collection it runs `just gh-analyze-bench-history` (`analyze --engine all --target-triple
  all --machine-key all` across every platform's history) which writes a Markdown report
  plus `bench-history-notable.txt`. When the JSON report's `notable` is `true`, it files
  **one rolling regression issue** via `JasonEtco/create-an-issue` (`update_existing` +
  `search_existing: open`, fixed title = dedup key). Findings never fail the job —
  the tool always exits 0; the issue is advisory. Needs `issues: write`. It checks out
  with `fetch-depth: 0` so the first-parent history resolves.
- **`alert` job** (`needs: [collect, analyze]`, `if: failure()` + main-only) — opens a
  deduplicated `.github/bench-history-failure-issue.md` when any prior job fails, so a
  broken nightly is noticed. The issue title comes from the workflow-level
  `FAILURE_ISSUE_TITLE` env constant (the template renders `{{ env.FAILURE_ISSUE_TITLE }}`),
  which is also the title create-an-issue dedups on. Its companion `resolve` job closes that
  issue automatically once the workflow is green again.
- **`resolve` job** (`needs: [collect, analyze]`, closes on success + main-only) — mirror of
  `alert`: when collection and analysis both pass, it finds any still-open failure issue
  (listed by the `ci-failure` label, then exact-matched against the shared
  `FAILURE_ISSUE_TITLE` constant) and closes it via the `gh` CLI with a comment linking the
  green run, so a fixed nightly does not leave a stale alert open. A no-op on nights when no
  failure issue is open. Its gate is the explicit `needs.collect.result == 'success' &&
  needs.analyze.result == 'success'` (rather than the bare `success()` function) so a future
  unrelated job cannot affect the decision; when `collect` or `analyze` actually failed those
  cases fall to `alert` instead. The advisory regression issue (label `regression`) is out of
  scope and stays manual.

## Design Decisions

1. **Concurrency control** — Workflows triggered by push or pull request use the `concurrency`
   key with `group: ${{ github.workflow }}-${{ github.head_ref || github.ref }}` and
   `cancel-in-progress: true`. This automatically cancels in-progress runs when new commits
   are pushed to the same PR branch or to main, avoiding wasted runner time on outdated code.
   The `cache-warmup` and `bench-history` workflows are excluded because they are
   schedule-triggered and not commit-driven.

2. **Parallelization over sequential execution** — Individual jobs provide:
   - Faster CI feedback (first failure visible immediately)
   - Clear failure identification (specific check names in GitHub status)
   - Better resource utilization across GitHub runners

   Expensive jobs are gated behind cheaper equivalents so a fast failure short-circuits
   the expensive work and avoids burning runner time on code that already failed a
   simpler check:
   - `check-release` depends on `check-dev`
   - `clippy-release` depends on `clippy-dev`
   - `build-release` depends on `check-dev` (no point linking a release binary if the
     dev `cargo check` already failed)
   - `check-frozen` depends on `check-dev` (checking the frozen minimum dependency
     versions is pointless if the code does not even build with the normal lockfile
     versions)
   - `miri-x64` and `miri-arm` depend on `check-dev` (Miri is much slower than `cargo check`;
     if the code does not even compile in dev mode, there is nothing for Miri to interpret)
   - `miri-x64` also depends on `test-more-x64`, and `miri-arm` also depends on
     `test-more-arm` (there is no point running the slow Miri interpreter unless the tests
     already pass in their base configuration)
   - `mutants` depends on `test-more-x64` (mutation testing is meaningless if base tests fail)
   - `careful` depends on `test-more-x64` (running the slow `cargo careful` test pass is
     pointless unless the tests already pass in their base configuration)
   - `miri-harder-*` depend on both `miri-x64` and `miri-arm` (many-seeds runs are
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

The `start-azurite` composite action (`.github/actions/start-azurite`) installs the Azurite
blob emulator via `npm install -g azurite` and starts it on the runner host at
`127.0.0.1:10000`, blocking until the port accepts connections. It is Linux-only (uses bash and
`/dev/tcp`) and is used only by the `test-azurite` job to back the `cargo-bench-history` Azure
storage backend tests. Node.js/npm are preinstalled on the GitHub-hosted Ubuntu runners.
