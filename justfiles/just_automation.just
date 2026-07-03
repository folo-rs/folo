# Recipes intended ONLY for automation (GitHub Actions), not local use. Each is
# prefixed `gh-` to signal that. They assume the workspace's committed
# `.cargo/bench_history.toml` already points at the prod Azure history store
# (account folohistory) and that the runner is signed in to Azure. We may factor
# these out into dedicated GitHub Actions later (see folo-rs/folo#284), so keep them
# thin wrappers over the tool, free of local-developer conveniences.

# Collects the whole workspace's benchmark history (excluding the slow, special-purpose
# `benchmarks` package) into the prod Azure store, keyed by the current commit. The
# machine partition for noisy wall-clock engines is auto-detected; deterministic engines
# (Callgrind, allocation/time counters) are hardware-independent. `--skip-existing` makes a
# re-run on an unchanged commit a no-op append (each already-stored object is skipped, not
# overwritten), so a re-triggered run of an already-collected commit never bumps the
# cache-invalidation marker the `analyze` job's read-through cache depends on — a re-run of
# the same commit is not "better" data, so there is nothing to replace.
[group('automation')]
gh-collect-bench-history:
    cargo run -p cargo-bench-history --bin cargo-bench-history --locked -- collect --workspace --exclude benchmarks --skip-existing --verbose

# Analyzes the collected history for regressions across ALL engines, target triples and
# machine keys, writing two working-directory artefacts: bench-history-report.md (the
# Markdown report) and bench-history-notable.txt (`true`/`false`, the JSON `notable`
# flag). Findings never fail the recipe (the tool always exits 0); the workflow files a
# rolling issue when notable. `auto` mode on a clean `main` checkout resolves to
# long-range history-trend detection. A single analyze pass emits both the Markdown and
# JSON reports via the per-format output toggles (`--no-text --markdown --json`).
#
# `--cache` mirrors fetched cloud objects under `bench-history-cache/` (relative to the
# repo root, the working directory here) so the bulk history is downloaded at most once
# across runs instead of in full every run. The bench-history.yml `analyze` job persists
# that directory between runs via `actions/cache`; its `path:` MUST match the directory
# named here. The cloud history is append-only (collection uses
# `--skip-existing`), so the cache only ever tops up — a stale mirror is wiped via the
# cloud-side marker only after a deliberate `--overwrite`/`prune`.
[group('automation')]
[script]
gh-analyze-bench-history:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    $reportPath = Join-Path (Get-Location) "bench-history-report.md"
    $jsonPath = Join-Path (Get-Location) "bench-history-report.json"
    $notablePath = Join-Path (Get-Location) "bench-history-notable.txt"
    $common = @('--engine', 'all', '--target-triple', 'all', '--machine-key', 'all')

    Write-Verbose "Analyzing benchmark history in a single pass; writing Markdown to $reportPath and JSON to $jsonPath, mirroring fetched cloud objects under bench-history-cache/ so the history is downloaded at most once across runs." -Verbose
    cargo run -p cargo-bench-history --bin cargo-bench-history --locked -- analyze @common --cache=bench-history-cache --no-text --markdown $reportPath --json $jsonPath
    Write-Verbose "Computing the notable flag from the JSON report." -Verbose
    $json = Get-Content -Path $jsonPath -Raw | ConvertFrom-Json
    $notable = if ($json.notable) { 'true' } else { 'false' }
    Set-Content -Path $notablePath -Value $notable -Encoding utf8
    Write-Verbose "Analysis complete: notable=$notable (report at $reportPath)." -Verbose

# The release-automation logic lives in scripts/release/ReleaseAutomation.psm1, with a Pester
# suite (scripts/release/ReleaseAutomation.Tests.ps1) that exercises it against fixtures via
# `just test-scripts`. The recipes below are thin wrappers that import the module and call its
# functions, which keeps the logic (crate derivation, config injection, retry loop, binary
# reconciliation, and the triple -> runner mapping) testable on a developer PC rather than only
# via a push to `main`.

# Writes a CI-only copy of release-plz.toml to {{output}} with `git_release_enable = true`
# set for each publishable binary crate (derived from cargo metadata), leaving everything
# else untouched. The committed config keeps releases OFF at the workspace level so the many
# library/`_impl` crates never spawn GitHub releases; enabling it only for the derived binary
# crates means a newly-added binary tool is covered with no config edit and no library crate
# is ever released. The caller writes this OUTSIDE the working tree so `cargo publish` never
# sees a dirty repo. See .github/workflows/release.yml and docs/release-automation.md.
[group('automation')]
[script]
gh-compose-release-config output:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true
    Import-Module ./scripts/release/ReleaseAutomation.psm1 -Force

    $crates = @(Get-PublishableBinaryCrate)
    Write-Verbose "Publishable binary crates: $($crates.Name -join ', ')" -Verbose
    New-ReleasePlzConfig -SourcePath 'release-plz.toml' -OutputPath '{{output}}' -CrateName $crates.Name
    Write-Verbose "Wrote CI release-plz config to {{output}}." -Verbose

# Publishes changed crates to crates.io via release-plz using the composed CI config, with
# bounded retries (3 attempts, 15 minutes apart) to ride out crates.io rate limits and the
# short-lived OIDC token. release-plz is idempotent — it re-checks the registry and skips
# already-published versions — so a retry (or a whole re-run) safely resumes a partially
# published release, and each attempt mints a fresh OIDC token. NOT for local use: it
# performs real publishes. See docs/release-automation.md.
[group('automation')]
[script]
gh-release config:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true
    Import-Module ./scripts/release/ReleaseAutomation.psm1 -Force

    Invoke-ReleasePublish -ConfigPath '{{config}}'

# Reconciles the desired prebuilt-binary archives against what is already uploaded to each
# published binary crate's GitHub release, and emits the matrix of missing (crate, target)
# pairs still to build as the `matrix` / `has_binaries` step outputs (also printed). This is
# what makes retries self-healing: a re-run rebuilds only whatever is still missing, with no
# hardcoded crate list. Safe to run locally — with no releases yet it reports an empty
# matrix. See .github/workflows/release.yml and docs/release-automation.md.
[group('automation')]
[script]
gh-plan-release-binaries:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true
    Import-Module ./scripts/release/ReleaseAutomation.psm1 -Force

    $crates = @(Get-PublishableBinaryCrate)
    $rows = if ($crates.Count -gt 0) { @(Get-MissingBinaryMatrix -Crate $crates) } else { @() }

    Write-Host "Missing (crate, target) archives: $($rows.Count)"
    Set-GitHubOutput -Name 'matrix' -Value (ConvertTo-MatrixJson -Row $rows)
    Set-GitHubOutput -Name 'has_binaries' -Value $(if ($rows.Count -gt 0) { 'true' } else { 'false' })
