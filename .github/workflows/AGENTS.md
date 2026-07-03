# GitHub Workflows Design Rationale

Update this ".github/workflows/AGENT.md" file if you change the GitHub workflows.

## Toolchain versions

Rust toolchain versions are defined in `constants.env` (loaded via just's dotenv support) and
`rust-toolchain.toml`. All `just` commands reference these variables instead of hardcoding
version numbers. The GitHub workflows call `just install-tools` and `just <command>`, so
toolchain versions flow automatically from `constants.env` without any duplication in workflows.

## Shell conventions

Workflow `run:` steps use `shell: pwsh` (PowerShell 7 is available on every runner). Prefer
PowerShell over Bash, matching the repository-wide convention
([`docs/build-and-tooling.md`](../../docs/build-and-tooling.md)). Keep steps thin: put any
non-trivial logic in a PowerShell `[script]` `just` recipe the step calls, so the logic can be
run and tested locally instead of only by triggering the workflow. Logic complex enough to
warrant unit tests goes one level deeper, into a module under `scripts/` that the recipe
imports, so it can be covered by a Pester suite (see `scripts/release/ReleaseAutomation.psm1`
and `just test-scripts`). (The `setup-environment` composite action still uses Bash internally,
because it bootstraps the Linux system packages — including PowerShell itself — before `pwsh`
is available.)

## Overview

The CI workflows in this repository run individual `just` commands as separate parallel jobs instead of combined `validate-local` and `validate-extra-local` commands. This design provides faster feedback by parallelizing checks and clearer failure identification.

## Workflow Structure

### validation.yml

Split from the monolithic `just validate-local` into individual jobs:

- **format-check** — Runs only on `ubuntu-latest` (single platform)
  - Rationale: Code formatting rules are platform-agnostic
  - Commands: `cargo +nightly fmt --check`, `cargo sort-derives --check`

- **lint-workflows** and **test-scripts** — Runs only on `ubuntu-latest`, and
  **unconditionally**: unlike the package-scoped jobs below, these do not depend on the
  `delta` job and have no `skip_all` gate. Their inputs are not Cargo packages — the workflow
  files under `.github/workflows/` for `lint-workflows` (via `just lint-workflows`, actionlint)
  and the standalone scripts under `scripts/` for `test-scripts` (via `just test-scripts`,
  Pester) — so a PR touching only those files produces `skip_all=true` from the delta analysis
  and would otherwise be validated by nothing. Both are fast static/unit checks, so running
  them on every PR is cheap.

- **Multi-platform jobs** (run on ubuntu-latest, macos-latest, windows-latest):
  - check-dev
  - clippy-dev
  - **test-x64** — the x86_64 test pass (ubuntu-latest, windows-latest)
    - Runs the full test suite (unit, integration and example tests) **with coverage
      instrumentation**, plus the benchmark targets once (`test-benches`) to verify they
      do not panic. Coverage is therefore a *side effect* of the regular test run, not a
      separately re-executed pass — there is intentionally no standalone `coverage` job.
    - macOS is **not** in this matrix: GitHub's `macos-latest` runner is Apple Silicon
      (arm64), so macOS is exercised by `test-arm`. Keeping this job x86_64-only keeps its
      name accurate and stops the matrix drifting as the `-latest` labels change.
    - Runs on the **nightly** toolchain because coverage needs `llvm-tools-preview`, which
      `just install-tools` installs only for nightly. The **MSRV** test pass lives on
      `test-arm`, and MSRV compilation of every target is covered by `check-dev`.
    - Uploads both the coverage report (lcov) and the test results (junit) to Codecov.
    - Paired with `test-arm`; both carry an explicit `-x64`/`-arm` suffix for symmetry.
  - test-docs
  - **docs** — Multi-platform because conditional compilation affects generated documentation
  - miri-x64
  - **test-arm** — the ARM64 test pass (ubuntu-24.04-arm, windows-11-arm, macos-latest)
    - Exercises ARM-specific code paths (anything gated behind
      `cfg(target_arch = "aarch64")` or similar) which x86_64 runners never compile
      or execute. macOS lives here too because GitHub's `macos-latest` runner is Apple
      Silicon (arm64). Doubles as the **MSRV** test pass (`test-x64` runs on nightly so it
      can collect coverage). Platform-neutral code is also validated by the x86_64 matrix.
    - Collects no coverage: `llvm-tools-preview`-based instrumentation is gathered on
      x86_64 only (the `test-x64` job).
    - Paired with `test-x64`; both carry an explicit `-x64`/`-arm` suffix for
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
  - Runs the Azure storage backend tests against a live Azurite blob emulator (in
    `--oauth basic` mode over HTTPS, fed a locally-faked Entra token) and also
    collects coverage so the `azure.rs` network paths reach Codecov.
  - Named for the Azurite emulator it targets; its sibling `test-azure` runs the
    same backend against a real Azure account. Single-platform by choice, not
    limitation: the Azure Blob backend is OS-agnostic network I/O, so one platform
    fully covers it and Linux is the cheapest runner.
  - The Azure backend's network paths **self-skip** when
    no emulator is reachable, so the multi-platform test jobs
    (`test-x64`, `test-arm`) stay green without one. They run for real only in
    this job: Azurite is provisioned on the runner host (via the `start-azurite`
    composite action) and `BENCH_HISTORY_REQUIRE_AZURITE=1` turns an unreachable
    emulator into a hard failure so it can never silently skip every test.
  - The multi-platform `test-x64` job runs without an emulator and therefore
    cannot cover `azure.rs`; this job uploads an `azure`-flagged lcov report that
    Codecov merges with it (a line covered in any upload counts as covered).
  - Linux-only because Azurite runs directly on the host: GitHub service
    containers cannot reliably bind the emulator to a reachable address (the
    default image binds `127.0.0.1` inside the container and the service syntax
    cannot override the command), so the host-process approach is used instead.

- **test-azure** — single-platform (Linux) by choice, package-gated to `cargo-bench-history`
  - Additive sibling of `test-azurite`: runs the same Azure storage backend
    tests against a **real Azure Storage account** to exercise **real Microsoft
    Entra ID** signature validation that the emulator (which fakes the token) cannot
    — a real account with shared-key access disabled, reached over HTTPS.
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
    OIDC credential** — the credential the per-push prod collection (`bench-history.yml`)
    depends on — against the real test account. The `storage/github_oidc.rs` unit tests
    stub the HTTP exchange and `test-azure` takes the local-`az`
    `DeveloperToolsCredential` fallback, so without this job nothing proves the
    self-minting path actually round-trips real GitHub OIDC → Entra → Blob until the
    post-merge collection run.
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

- **coverage-notify** — single-platform gate that releases Codecov's coverage
  notifications once every coverage upload for the commit has landed.
  - Coverage for one commit is uploaded by several jobs: one upload per platform from
    the `test-x64` matrix, plus a conditional `azure`-flagged upload from `test-azurite`
    when `cargo-bench-history` is affected. The per-commit upload count is therefore
    variable. (`test-azure` / `test-azure-gh` upload no coverage.)
  - By default Codecov re-posts its commit status / PR comment after every upload, so an
    early upload makes it report a misleadingly low "coverage decreased" figure just
    because the remaining uploads have not arrived yet. The repo-root `codecov.yml` sets
    `codecov.notify.manual_trigger: true`, which holds ALL coverage notifications until
    the CLI `send-notifications` command runs; this job (`codecov/codecov-action@v7` with
    `run_command: send-notifications`) issues it once, so the status is computed from the
    complete set of uploads. `manual_trigger` is preferred over `after_n_builds` because
    the latter would need a hardcoded count that the conditional `test-azurite` upload
    breaks. `codecov.yml` also disables `wait_for_ci` / `require_ci_to_pass` so the status
    posts as soon as coverage data is complete rather than waiting on unrelated slow jobs
    (`mutants`, etc.); those jobs gate merges through their own checks.
  - `needs: [test-x64, test-azurite]`; `if: !cancelled() && needs.test-x64.result ==
    'success' && (needs.test-azurite.result == 'success' || needs.test-azurite.result ==
    'skipped')`. The `!cancelled()` status function lets it run even when `test-azurite`
    is skipped (without it, the skipped dependency would skip this job too). It releases
    notifications only when every expected upload landed: `test-x64` succeeded, and
    `test-azurite` either succeeded (azure upload landed) or was skipped (cargo-bench-history
    not affected, so no azure upload was expected). If `test-x64` failed, or `test-azurite`
    ran and failed, an expected upload is missing and the build is already red, so the gate
    does not release a status off an incomplete set. A `skip_all` run uploads no coverage at
    all, so `test-x64` is skipped and this job does not run.
  - Runs for fork PRs too: the token is empty there and the action falls back to
    tokenless notification (public repository), so external-contributor PRs get the same
    gated, complete-data coverage status.

### cache-warmup.yml

A scheduled workflow that keeps GitHub Actions caches warm. GitHub evicts caches after
7 days of inactivity, and a cold cache means every parallel validation job must independently
compile all Rust dependencies from scratch (the setup-environment step becomes very expensive).
This workflow runs once daily on all five runner images (ubuntu-latest, windows-latest,
macos-latest, ubuntu-24.04-arm, windows-11-arm) to ensure the
`shared-key: prerequisites` Rust cache is always populated for both x86_64 and ARM64. It also supports `workflow_dispatch`
for manual cache warming after toolchain updates.

### bench-history.yml

A push-triggered workflow that collects the workspace's benchmark results into the
long-lived Azure history store on every push to `main`, building the performance history
that `cargo-bench-history analyze` reads to detect regressions. It runs per-push rather
than on a nightly schedule: a nightly run re-benchmarks an unchanged tip for no gain and
misses intermediate same-day commits, whereas per-push collects exactly one data point per
commit. The accepted trade-off is more runner time on busy days.

- **Multi-platform matrix** (ubuntu-latest, windows-latest, ubuntu-24.04-arm,
  windows-11-arm) with `fail-fast: false`: each OS/architecture is a distinct
  measurement target, and a failure on one platform must not abandon the others'
  history for that commit. macOS is omitted — there is no macOS-hosted history store
  consumer yet; add it to the matrix if/when macOS performance tracking is wanted.
- **Whole workspace except the `benchmarks` package**, via the
  `just gh-collect-bench-history` recipe (`cargo-bench-history collect --workspace --exclude
  benchmarks --skip-existing`). The `benchmarks` package holds slow, special-purpose
  benchmarks that are not part of the tracked history. `--skip-existing` makes a re-run on
  an already-collected `main` commit a no-op append (each already-stored object is skipped,
  not overwritten) rather than failing as a duplicate — and, crucially, never bumps the
  cache-invalidation marker the `analyze` job's read-through cache depends on, so a
  re-triggered append-only run never wipes that cache.
- **Uses a dedicated prod managed identity** (`id-folo-bench-history-prod`, provisioned
  by `infra/azure-bench-history-prod/` alongside the account), not the test identity: a
  push (or gated dispatch) on `main` produces the OIDC subject
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
- **Same-repo + main-only gate** (`if: github.repository == 'folo-rs/folo' && github.ref ==
  'refs/heads/main'`): only this repository's identity can federate into Azure, and the
  federated credential is scoped to `refs/heads/main`, so a fork or a `workflow_dispatch`
  from a feature branch skips cleanly instead of failing at OIDC exchange.
- **Auto-detected machine key**: the wall-clock engines partition by the runner's
  auto-detected machine fingerprint (no override) — the `target-triple` already separates
  OS/arch, and an explicit key would risk merging dissimilar hosts under one partition.
- `timeout-minutes: 360` (a generous ceiling that only bounds a genuinely stuck run, since
  the matrix runs in parallel). Unlike `cache-warmup`, this workflow IS commit-driven, so it
  carries a `concurrency` block — but keyed on the commit **SHA**
  (`${{ github.workflow }}-${{ github.sha }}`), NOT the ref. Distinct commits must collect
  in parallel: each is its own history data point, and the ref-keyed cancel-in-progress used
  by `validation.yml` would cancel an in-flight commit's benchmarks whenever a newer commit
  landed, dropping that commit's data and defeating the per-commit goal. With a SHA key,
  `cancel-in-progress: true` only dedups a redundant re-trigger of the SAME commit (a re-run
  or a dispatch on an already-pushed commit), which `--skip-existing` would make a no-op
  append anyway.
- **`analyze` job** (`needs: collect`, `if: always()` + same main-only gate) — after
  collection it runs `just gh-analyze-bench-history` (`analyze --engine all --target-triple
  all --machine-key all` across every platform's history) which writes a Markdown report
  plus `bench-history-notable.txt`. When the JSON report's `notable` is `true`, it files
  **one rolling regression issue** via `JasonEtco/create-an-issue` (`update_existing` +
  `search_existing: open`, fixed title = dedup key). Findings never fail the job —
  the tool always exits 0; the issue is advisory. Needs `issues: write`. It checks out
  with `fetch-depth: 0` so the first-parent history resolves. An `actions/cache` step
  ("Restore benchmark-history read-through cache") persists the recipe's `--cache`
  directory (`bench-history-cache/`, the read-through mirror) between runs so the bulk
  history is downloaded at most once rather than in full every run. The cache key is unique
  per run (`bench-history-cache-<run_id>`; entries are immutable per key) with
  `restore-keys: bench-history-cache-`, so each run restores the most recent prior mirror,
  the analyze step tops it up with this run's new objects, and the run saves a fresh entry
  on success. The step's `path:` must stay in lockstep with the directory the recipe passes
  to `--cache`. Correctness does not depend on the cache being fresh: the cloud history is
  append-only (collect uses `--skip-existing`), and a deliberate `--overwrite`/`prune`
  bumps a cloud-side marker that wipes the mirror on the next read.
- **`alert` job** (`needs: [collect, analyze]`, `if: failure()` + main-only) — opens a
  deduplicated `.github/bench-history-failure-issue.md` when any prior job fails, so a
  broken run is noticed. The issue title comes from the workflow-level
  `FAILURE_ISSUE_TITLE` env constant (the template renders `{{ env.FAILURE_ISSUE_TITLE }}`),
  which is also the title create-an-issue dedups on. Its companion `resolve` job closes that
  issue automatically once the workflow is green again.
- **`resolve` job** (`needs: [collect, analyze]`, closes on success + main-only) — mirror of
  `alert`: when collection and analysis both pass, it finds any still-open failure issue
  (listed by the `ci-failure` label, then exact-matched against the shared
  `FAILURE_ISSUE_TITLE` constant) and closes it via the `gh` CLI with a comment linking the
  green run, so a fixed run does not leave a stale alert open. A no-op on runs when no
  failure issue is open. Its gate is the explicit `needs.collect.result == 'success' &&
  needs.analyze.result == 'success'` (rather than the bare `success()` function) so a future
  unrelated job cannot affect the decision; when `collect` or `analyze` actually failed those
  cases fall to `alert` instead. The advisory regression issue (label `regression`) is out of
  scope and stays manual.

### release.yml

A push-triggered workflow that publishes changed crates to crates.io and attaches
cargo-binstall prebuilt binaries, on every push to `main`. Version bumping is the only
manual step (`just prepare-release`, then commit and push); everything downstream is
automatic. `docs/release-automation.md` holds the full design; this section captures the
rationale that lives with the workflow.

The workflow steps are thin: the non-trivial logic lives in PowerShell `just` recipes in
[`justfiles/just_automation.just`](../../justfiles/just_automation.just)
(`gh-compose-release-config`, `gh-release`, `gh-plan-release-binaries`), which are themselves
thin wrappers over the [`scripts/release/ReleaseAutomation.psm1`](../../scripts/release/ReleaseAutomation.psm1)
module. That module is covered by a Pester suite (`just test-scripts`, gated in CI), so the
release logic can be run and tested locally rather than only exercised by pushing to `main`.
Every `run:` step uses `shell: pwsh`.

- **One workflow, not two.** Publishing and the binary build/upload jobs share a single
  run on purpose: a git tag or GitHub release created with the ambient `GITHUB_TOKEN`
  does **not** trigger downstream `on: release` / `on: push: tags` workflows. Driving the
  binary jobs from within the same run (rather than a separate tag-triggered workflow)
  is what lets the built-in token suffice — no PAT or GitHub App token is needed.
- **`publish` job — Trusted Publishing + bounded retries.** Calls `just gh-release`, which
  runs `release-plz release`. crates.io Trusted Publishing is auto-detected from
  `permissions: id-token: write` with no `CARGO_REGISTRY_TOKEN` present: release-plz
  exchanges a GitHub OIDC token for a short-lived crates.io token itself, so no long-lived
  registry secret is stored. The recipe retries up to three times, 15 minutes apart,
  because crates.io rate-limits and the OIDC token is short-lived. release-plz is idempotent
  (it re-checks the registry and publishes only versions not already there), so a retry — or
  a whole re-run — safely resumes a partially-published release, and each attempt mints a
  fresh OIDC token so a publish that overruns the token lifetime simply fails that attempt
  and the next proceeds. `GIT_TOKEN` (the built-in `GITHUB_TOKEN`) lets release-plz push
  tags and create releases.
- **Dynamic `git_release_enable` injection.** The committed `release-plz.toml` keeps
  `git_release_enable = false` (most crates are libraries and must not get GitHub
  releases). Before releasing, `just gh-compose-release-config` derives the publishable
  **binary** crates from `cargo metadata` (publishable AND owning a `bin` target) and writes
  a CI-only copy of the config with `git_release_enable = true` set on exactly those crates,
  passed via `--config`. The copy is written under `$RUNNER_TEMP` (outside the working
  tree) so `cargo publish` never sees a dirty repo. Because the enabled set is always
  derived, a newly-added binary crate is covered with no config edit and no library crate
  is ever released.
- **`plan-binaries` job — self-healing reconciliation.** Calls `just
  gh-plan-release-binaries`, which runs after every successful publish, **not** gated on
  whether anything was published this run. It re-derives the publishable binary crates at
  their current manifest versions, and for each computes the expected tag `{name}-v{version}`
  and queries the release's existing assets (`gh release view`). For every (crate, target)
  whose `{name}-v{version}-{triple}.zip` archive is missing, it emits a matrix entry (as the
  step's `matrix` / `has_binaries` outputs). This is why retries heal: a plain re-run or a
  bare `workflow_dispatch` reconciles the actually-published state against uploaded assets
  and rebuilds only what is missing — never a hand-maintained crate list. A matrix keyed off
  "published this run" could not do this, because a re-run skips the already-published crate
  and it drops out of release-plz's output.
- **`build-binaries` job — native per-target archives.** `if:
  needs.plan-binaries.outputs.has_binaries == 'true'`, `fail-fast: false` so one target's
  failure does not abandon the others (the next run's reconciliation rebuilds whatever is
  still missing). Each matrix entry checks out the released **tag** (not the branch tip)
  and uses `taiki-e/upload-rust-binary-action`, which builds, packages, checksums
  (`sha256`) and uploads to the crate's release. Its `ref:` is set to the crate's tag so
  the asset lands on the right release rather than on `github.ref` (the branch). Archives
  are `.zip` on **every** platform (`tar: none`, `zip: all`) so each crate's
  `[package.metadata.binstall]` block needs no per-OS format override.
- **Target matrix (triple → runner).** `x86_64-unknown-linux-gnu` → `ubuntu-latest`,
  `aarch64-unknown-linux-gnu` → `ubuntu-24.04-arm`, `x86_64-pc-windows-msvc` →
  `windows-latest`, `aarch64-pc-windows-msvc` → `windows-11-arm`, `aarch64-apple-darwin`
  → `macos-latest` (macos-latest is arm64). Native runners, one per target, no
  cross-compilation. The ARM Linux/Windows images are pinned by version because GitHub
  offers no `-latest` alias for them; bump the pins when newer images ship. Intel macOS is
  intentionally not built.
- **Standard environment.** `publish`, `plan-binaries` and `build-binaries` all use the
  shared `setup-environment` composite action (which installs `just`, `pwsh`, the Rust
  toolchain and release-plz) rather than a bespoke minimal toolchain: deviating from the
  standard environment causes more trouble than the (mostly cached) setup time it saves,
  and every job needs `just` to call its recipe. The trivial `alert` job is the exception —
  it only needs the preinstalled `gh` and `pwsh`, so it skips setup to alert quickly.
- **`alert` job — one issue per failed run.** `needs: [publish, plan-binaries,
  build-binaries]`, `if: failure() && github.repository == 'folo-rs/folo'`. It files a
  GitHub issue whose title includes the run id (so every failed release is tracked
  individually, not folded into a rolling issue), labelled `ci-failure` (the shared label,
  ensured to exist idempotently first). `failure()` fires only on a real failure — a
  skipped `build-binaries` (nothing to rebuild) does not count — and the issue body tells
  the operator that simply re-running the workflow resumes publishing and heals missing
  binaries.
- **`concurrency`** keyed on the ref with `cancel-in-progress: false`: a release must
  never be cancelled mid-flight, so a later push queues behind the in-flight run.
- **New-crate caveat handled at prepare time, not here.** crates.io Trusted Publishing
  cannot perform the *first-ever* publish of a brand-new crate (the crate must exist so
  its Trusted Publishing config can be set). `just prepare-release` detects never-published
  crates against the sparse index and warns that the first release must be done manually
  (a plain `cargo publish`), after which Trusted Publishing is configured and subsequent
  releases flow through this workflow.

## Design Decisions

1. **Concurrency control** — Workflows triggered by push or pull request use the `concurrency`
   key with `group: ${{ github.workflow }}-${{ github.head_ref || github.ref }}` and
   `cancel-in-progress: true`. This automatically cancels in-progress runs when new commits
   are pushed to the same PR branch or to main, avoiding wasted runner time on outdated code.
   The `cache-warmup` workflow is excluded because it is schedule-triggered and not
   commit-driven. `bench-history` is commit-driven (push to main) and so DOES carry a
   `concurrency` block, but keyed on the commit **SHA** rather than the ref: it collects one
   history data point per commit, so cancelling an in-flight commit to favour a newer one
   would drop measurements. See its section above for the full rationale.

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
   - `miri-x64` also depends on `test-x64`, and `miri-arm` also depends on
     `test-arm` (there is no point running the slow Miri interpreter unless the tests
     already pass in their base configuration)
   - `mutants` depends on `test-x64` (mutation testing is meaningless if base tests fail)
   - `careful` depends on `test-x64` (running the slow `cargo careful` test pass is
     pointless unless the tests already pass in their base configuration)
   - `miri-harder-*` depend on both `miri-x64` and `miri-arm` (many-seeds runs are
     orders of magnitude slower than a single Miri pass)

3. **Platform matrix considerations**:
   - `format-check` is single-platform (Ubuntu) to save resources
   - `docs` and `machete` remain multi-platform due to conditional compilation differences
   - `miri-harder-*` jobs are Windows-only due to high cost; sharded across parallel runners
   - Most checks run on the 3 x86_64 platforms (ubuntu, macos, windows)
   - `test-arm` and `miri-arm` extend coverage to ARM64 (ubuntu-24.04-arm,
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

6. **Codecov notification gating** — Coverage is uploaded by a variable number of jobs
   per commit (the `test-x64` platform matrix plus the conditional `test-azurite` azure
   upload). To stop Codecov posting a misleading status off a partial set of uploads,
   the repo-root `codecov.yml` enables `codecov.notify.manual_trigger` and the
   `coverage-notify` job releases notifications via the CLI `send-notifications` command
   only once all coverage-producing jobs have finished. See the `coverage-notify` job
   description above for details.

## Maintenance Notes

- When adding new checks, consider platform-specific needs and appropriate timeouts
- Keep inline comments for single-platform jobs to explain the rationale
- Monitor job run times and adjust timeouts if needed

## System dependencies

The `setup-environment` action installs Valgrind on Linux runners and `gungraun-runner` (via
`just install-tools`). Both are required for the Callgrind `*_cg` bench targets that
live in `packages/*/benches/`. Even when those targets are not executed, `cargo test
--benches` (which `test-x64` and `test-arm` use, via the `test-benches` recipe) invokes each
bench binary's `main()` once, which forwards
its args to `gungraun-runner`. Without `gungraun-runner` on `$PATH` the call fails with
`Failed to run benchmarks: No such file or directory`, which breaks `test-x64` and
`test-arm`. The Callgrind bench binaries are compiled to no-op stubs on Windows and macOS via
`#[cfg(not(target_os = "linux"))] fn main() {}`, so neither tool is required there.

The `start-azurite` composite action (`.github/actions/start-azurite`) installs the Azurite
blob emulator via `npm install -g azurite`, generates a throwaway self-signed certificate,
and starts it on the runner host at `https://127.0.0.1:10000` in `--oauth basic` mode
(Entra is the only supported auth mode and requires TLS), blocking until the port accepts
connections. It is Linux-only (uses bash and `/dev/tcp`) and is used only by the
`test-azurite` job to back the `cargo-bench-history` Azure storage backend tests.
Node.js/npm are preinstalled on the GitHub-hosted Ubuntu runners.
