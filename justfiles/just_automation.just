# Recipes intended ONLY for automation (GitHub Actions), not local use. Each is
# prefixed `gh-` to signal that. They assume the workspace's committed
# `.cargo/bench_history.toml` already points at the prod Azure history store
# (account folohistory) and that the runner is signed in to Azure. We may factor
# these out into dedicated GitHub Actions later (see folo-rs/folo#284), so keep them
# thin wrappers over the tool, free of local-developer conveniences.

# Collects the whole workspace's benchmark history (excluding the slow, special-purpose
# `benchmarks` package) into the prod Azure store, keyed by the current commit. The machine
# partition for noisy wall-clock engines is pinned to the fixed key `github` rather than
# auto-detected: the GitHub-hosted runner pool may contain differently-specced machines, and a
# per-machine fingerprint would fork the series (and split the history) whenever a run lands on
# a different SKU. Pinning keeps every push on one series (accepting the jitter a heterogeneous
# pool causes); a genuine hardware migration is absorbed by blessing the resulting step change.
# Deterministic engines (Callgrind, allocation/time counters) are hardware-independent and
# ignore the key. `--skip-existing` makes a re-run on an unchanged commit a no-op append (each
# already-stored object is skipped, not overwritten), so a re-triggered run of an
# already-collected commit never bumps the cache-invalidation marker the `analyze` job's
# read-through cache depends on — a re-run of the same commit is not "better" data, so there is
# nothing to replace.
[group('automation')]
gh-collect-bench-history:
    cargo run -p cargo-bench-history --bin cargo-bench-history --locked -- collect --workspace --exclude benchmarks --machine-key github --skip-existing --verbose

# Analyzes the collected history for regressions across ALL engines and target triples, but
# only the `github` machine key — the fixed key `gh-collect-bench-history` stamps on the
# GitHub runner pool's data. Restricting it here (rather than `--machine-key all`) keeps this
# job surveying only GitHub-pool history and never mixes in a stray key some other machine may
# have inserted into the shared store. It writes four artefacts INTO the given {{dir}}:
# report.md (the full Markdown report), report.json (the full JSON report), summary.md (the
# Markdown report condensed to the top findings by magnitude, so the rolling regression issue's
# body stays under GitHub's 65,536-character limit) and notable.txt (`true`/`false`, the JSON
# `notable` flag). Findings never fail the recipe (the tool always exits 0); the workflow files
# a rolling issue when notable, posting the summary as the body and attaching the two full
# reports as run artifacts. `auto` mode on a clean `main` checkout resolves to long-range
# history-trend detection. A single analyze pass emits every report via the per-format output
# toggles (`--no-text --markdown --json --markdown-summary`).
#
# {{dir}} is a scratch directory OUTSIDE the repo checkout (the bench-history.yml `analyze`
# job passes the runner temp dir). Keeping every artefact out of the working tree is what
# stops `analyze`'s own `git status --porcelain` dirty-check from seeing these files and
# wrongly annotating the analyzed tip `+ uncommitted changes` on an otherwise-clean `main`.
#
# `--cache` mirrors fetched cloud objects under `{{dir}}/cache` so the bulk history is
# downloaded at most once across runs instead of in full every run. The bench-history.yml
# `analyze` job persists that directory between runs via `actions/cache`; its `path:` and the
# report/summary/notable filenames its steps read MUST match the names written here. The cloud
# history is append-only (collection uses `--skip-existing`), so the cache only ever tops up —
# a stale mirror is wiped via the cloud-side marker only after a deliberate `--overwrite`/`prune`.
[group('automation')]
[script]
gh-analyze-bench-history dir:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    $dir = "{{dir}}"
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
    $reportPath = Join-Path $dir "report.md"
    $jsonPath = Join-Path $dir "report.json"
    $summaryPath = Join-Path $dir "summary.md"
    $notablePath = Join-Path $dir "notable.txt"
    $cacheDir = Join-Path $dir "cache"
    $common = @('--engine', 'all', '--target-triple', 'all', '--machine-key', 'github')

    Write-Verbose "Analyzing benchmark history in a single pass; writing the full Markdown to $reportPath, the full JSON to $jsonPath and the condensed top-findings Markdown summary to $summaryPath, mirroring fetched cloud objects under $cacheDir so the history is downloaded at most once across runs." -Verbose
    cargo run -p cargo-bench-history --bin cargo-bench-history --locked -- analyze @common --cache=$cacheDir --no-text --markdown $reportPath --json $jsonPath --markdown-summary $summaryPath
    Write-Verbose "Computing the notable flag from the JSON report." -Verbose
    $json = Get-Content -Path $jsonPath -Raw | ConvertFrom-Json
    $notable = if ($json.notable) { 'true' } else { 'false' }
    Set-Content -Path $notablePath -Value $notable -Encoding utf8
    Write-Verbose "Analysis complete: notable=$notable (full report at $reportPath, condensed summary at $summaryPath)." -Verbose

# Files (or updates in place) a single rolling issue identified by its exact title, from a
# pre-rendered body file. Thin wrapper over Publish-RollingIssue in
# scripts/bench-history/BenchHistoryIssue.psm1 (Pester-tested via `just test-scripts`), the one
# way this workflow opens issues now that JasonEtco/create-an-issue is gone: `gh` reads the body
# from any path (--body-file), so it can live in the runner temp dir, outside the repo checkout.
# The title is the dedup key — a persisting condition updates one rolling issue instead of opening
# a new one each run — so both the regression alert ('Benchmark regressions detected') and the
# workflow-failure alert route through here with their own title/label. `label` is the
# comma-separated label list applied on creation. Needs GH_TOKEN in the environment (the workflow
# supplies it).
[group('automation')]
[script]
gh-file-rolling-issue title label body_file:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true
    Import-Module ./scripts/bench-history/BenchHistoryIssue.psm1 -Force

    Write-Verbose "Filing or updating the rolling issue titled '{{title}}' (labels '{{label}}') from body file {{body_file}}." -Verbose
    $url = Publish-RollingIssue `
        -Title '{{title}}' `
        -Label '{{label}}' `
        -BodyFile '{{body_file}}' `
        -Verbose
    Write-Verbose "Rolling issue is available at $url." -Verbose

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

    # Print the composed config so the CI log records exactly what release-plz will run with
    # (which crates got `git_release_enable = true`), without needing to unpack the runner temp.
    Write-Host "----- Composed CI release-plz config ({{output}}) -----"
    Get-Content -Path '{{output}}' -Raw | Write-Host
    Write-Host "----- End composed CI release-plz config -----"

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

    # Get-MissingBinaryMatrix emits its decision history (per crate: expected tag, whether the
    # release exists, and the per-target present/missing verdict) via Write-Verbose. This is an
    # automation-only recipe, so always emit it: the reasoning is worth capturing in every run
    # (CI log or a local diagnostic invocation), not just the final count.
    $crates = @(Get-PublishableBinaryCrate)
    Write-Verbose "Publishable binary crates: $($crates.Name -join ', ')" -Verbose
    $rows = if ($crates.Count -gt 0) { @(Get-MissingBinaryMatrix -Crate $crates -Verbose) } else { @() }

    Write-Host "Missing (crate, target) archives: $($rows.Count)"
    Set-GitHubOutput -Name 'matrix' -Value (ConvertTo-MatrixJson -Row $rows)
    Set-GitHubOutput -Name 'has_binaries' -Value $(if ($rows.Count -gt 0) { 'true' } else { 'false' })
