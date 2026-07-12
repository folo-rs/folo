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
# ignore the key. `--best-of 3` counteracts the same runner noise from the other direction: the
# suite runs three times and each metric keeps its minimum sample, since benchmark interference
# on a shared runner is one-sided (it only ever makes a case slower) and the runs are spaced far
# enough apart in time that a transient slowdown is unlikely to hit every one. This roughly
# triples the wall-clock benchmark portion of the collect job (see bench-history.yml's
# `timeout-minutes`). `--skip-existing` makes a re-run on an unchanged commit a no-op append (each
# already-stored object is skipped, not overwritten), so a re-triggered run of an
# already-collected commit never bumps the cache-invalidation marker the `analyze` job's
# read-through cache depends on - a re-run of the same commit is not "better" data, so there is
# nothing to replace.
#
# RECOLLECT MODE. When the environment variable `RECOLLECT_COMMIT_ID` is set (the bench-history.yml
# `collect` job wires it from the optional `recollect_commit_id` workflow_dispatch input), this
# recipe instead re-measures that ONE historical commit and OVERWRITES its stored point, rather
# than appending the pushed commit. It does so with `backfill <id> <id> --overwrite`, which
# benchmarks the code AT that commit inside a throwaway git worktree (never the primary checkout)
# while running THIS (HEAD) build of the tool - so a data point corrupted by a bad benchmark day
# (transient spikes, a degraded runner) is replaced by a clean re-run under the CURRENT collection
# logic, never the tool version that happened to ship at that commit. `--overwrite` bumps the
# cloud cache-invalidation marker so the `analyze` job's read-through cache refreshes; note it also
# drops any blessings recorded at that commit, which must be re-applied if the re-measured level is
# still an intentional step change. The value should be a real commit on `main`'s first-parent
# history: the module validates only the SHA *format*, and `backfill` then requires the id to
# *resolve* to a commit in the clone (failing loudly otherwise) - but neither checks main
# ancestry, so a resolvable off-main commit is not rejected; it would just overwrite/store a
# point the `analyze` job (which surveys only `main`) never surfaces. The mode selection and the
# untrusted-input validation live in
# scripts/bench-history/BenchHistoryCollect.psm1 (Pester-tested via `just test-scripts`), so this
# recipe is a thin import + call; the value is read from the environment, never spliced into the
# script body.
[group('automation')]
[script]
gh-collect-bench-history:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    Import-Module ./scripts/bench-history/BenchHistoryCollect.psm1 -Force

    # RECOLLECT_COMMIT_ID is empty on a push / plain dispatch (normal append) and a commit SHA when
    # repairing one historical point (overwrite). The mode-selection logic - and the untrusted-input
    # validation - lives in the module so it is Pester-tested (BenchHistoryCollect.Tests.ps1); this
    # step is a thin wrapper that runs whatever subcommand + flags it returns.
    $toolArgs = Get-BenchHistoryCollectCommand -RecollectCommitId $env:RECOLLECT_COMMIT_ID -Verbose
    cargo run -p cargo-bench-history --bin cargo-bench-history --locked -- @toolArgs

# The PR counterpart of `gh-collect-bench-history`: collects benchmark history for ONLY the
# packages a pull request touched (the pr-bench-history.yml `delta` job computes them from
# cargo-delta and passes them here space-separated, already filtered to the benchmarkable set), into
# the SAME prod Azure store, keyed by the PR head commit. Scoping to the touched packages is a pure
# cost optimisation - a PR rarely changes every crate, and the full 4-platform matrix is expensive -
# and it is safe because branch-mode `analyze` only ever flags a series that has a data point at the
# branch-unique head commit, i.e. exactly the packages collected here (see
# gh-analyze-pr-bench-history and .github/workflows/design.md). Everything else is identical to the
# push-to-main collect: the fixed `github` machine key keeps PR wall-clock series comparable to
# main's baseline, `--best-of 3` sheds one-sided runner jitter, and `--skip-existing` makes a re-run
# of the same head SHA a no-op append. There is no recollect mode on PRs (that repairs main's
# permanent history); the scope decision lives in
# scripts/bench-history/BenchHistoryCollect.psm1 (Pester-tested via `just test-scripts`), so this is
# a thin import + call. `packages` must be non-empty - the workflow only invokes this recipe when
# the delta produced at least one benchmarkable package, and an empty scope would wrongly fall back
# to a whole-workspace run, so guard against it here.
[group('automation')]
[script]
gh-collect-pr-bench-history packages:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    Import-Module ./scripts/bench-history/BenchHistoryCollect.psm1 -Force

    # The delta job passes the touched packages space-separated (the same shape validation.yml's
    # `just package="..."` gating uses); split them into the array the module scopes on.
    $packageList = @('{{packages}}'.Split([char[]]@(' ', "`t", "`n"), [StringSplitOptions]::RemoveEmptyEntries))
    if ($packageList.Count -eq 0) {
        throw "gh-collect-pr-bench-history requires at least one package to collect; got an empty list. The pr-bench-history workflow must gate this recipe on a non-empty benchmarkable delta."
    }

    Write-Verbose "Collecting PR benchmark history scoped to the touched packages: $($packageList -join ', ')." -Verbose
    $toolArgs = Get-BenchHistoryCollectCommand -RecollectCommitId '' -Package $packageList -Verbose
    cargo run -p cargo-bench-history --bin cargo-bench-history --locked -- @toolArgs

# Analyzes the collected history for regressions across ALL engines and target triples, but
# only the `github` machine key - the fixed key `gh-collect-bench-history` stamps on the
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
# history is append-only (collection uses `--skip-existing`), so the cache only ever tops up -
# a stale mirror is wiped via the cloud-side marker only after a deliberate `--overwrite`/`prune`.
[group('automation')]
[script]
gh-analyze-bench-history dir:
    Set-StrictMode -Version Latest
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
    $notable = if (($json.PSObject.Properties.Name -contains 'notable') -and $json.notable) { 'true' } else { 'false' }
    Set-Content -Path $notablePath -Value $notable -Encoding utf8
    Write-Verbose "Analysis complete: notable=$notable (full report at $reportPath, condensed summary at $summaryPath)." -Verbose

# The PR counterpart of `gh-analyze-bench-history`: analyzes the collected history in BRANCH mode -
# comparing the PR head's benchmark level against its base branch - and writes the same four
# artefacts (report.md, report.json, summary.md, notable.txt) INTO {{dir}}. Branch mode is what the
# tool auto-selects when the context (the PR head, defaulting to HEAD) differs from {{base}}: it
# splits the head's first-parent ancestry at the merge-base with the base, treats the base
# ancestry's clean points as the baseline, and reports how the branch tip's level moved. `{{base}}`
# is passed EXPLICITLY (the pr-bench-history.yml `analyze` job passes `origin/main`) because the PR
# head is checked out detached, so there is no local `main` branch to auto-resolve, whereas
# `fetch-depth: 0` populates the `origin/main` remote-tracking ref the merge-base needs.
# `--include-improvements` is added so the rolling PR comment can also surface detected speedups, not
# only regressions. Everything else matches the main analyze: only the `github` machine key (the
# fixed key collection stamps on the runner pool), all engines and triples, findings never fail the
# recipe (the tool always exits 0), and `--cache` mirrors fetched cloud objects under {{dir}}/cache.
#
# This is intentionally NOT package-scoped even though collection is: `analyze` has no per-package
# filter (only positional id-prefix subjects), and benchmark ids are engine-dependent (Callgrind
# carries the package, Criterion group ids are crate-prefixed, but alloc_tracker/all_the_time use a
# bare operation name), so an id-prefix filter would silently drop the measurement-engine series for
# a touched package. The scoping is instead enforced by `analyze`'s DEFAULT ghost exclusion: it
# analyzes only benchmarks present at the context commit (HEAD, the PR head), and only the collected
# (touched) packages have a run there, so every untouched package is dropped as a "ghost" before
# detection - for EVERY engine. This recipe therefore MUST NOT pass `--include-ghosts`: that flag
# re-admits every historical benchmark in the store and would defeat the scoping. As a bonus the
# same default also drops a benchmark this PR removed, so a deletion is never mis-flagged as a
# regression. See .github/workflows/design.md for the full invariant.
#
# {{dir}} is a scratch directory OUTSIDE the repo checkout (the `analyze` job passes the runner temp
# dir), so `analyze`'s own `git status --porcelain` dirty-check never sees these artefacts.
[group('automation')]
[script]
gh-analyze-pr-bench-history dir base:
    Set-StrictMode -Version Latest
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

    Write-Verbose "Analyzing PR benchmark history in branch mode (context HEAD vs base {{base}}); writing the full Markdown to $reportPath, the full JSON to $jsonPath and the condensed top-findings Markdown summary to $summaryPath, including improvements as well as regressions, and mirroring fetched cloud objects under $cacheDir." -Verbose
    cargo run -p cargo-bench-history --bin cargo-bench-history --locked -- analyze @common --context HEAD --base '{{base}}' --include-improvements --cache=$cacheDir --no-text --markdown $reportPath --json $jsonPath --markdown-summary $summaryPath
    Write-Verbose "Computing the notable flag from the JSON report." -Verbose
    $json = Get-Content -Path $jsonPath -Raw | ConvertFrom-Json
    $notable = if (($json.PSObject.Properties.Name -contains 'notable') -and $json.notable) { 'true' } else { 'false' }
    Set-Content -Path $notablePath -Value $notable -Encoding utf8
    Write-Verbose "PR analysis complete: notable=$notable (full report at $reportPath, condensed summary at $summaryPath)." -Verbose

# Files (or updates in place) a single rolling issue identified by its exact title, from a
# pre-rendered body file. Thin wrapper over Publish-RollingIssue in
# scripts/bench-history/BenchHistoryIssue.psm1 (Pester-tested via `just test-scripts`), the one
# way this workflow opens issues now that JasonEtco/create-an-issue is gone: `gh` reads the body
# from any path (--body-file), so it can live in the runner temp dir, outside the repo checkout.
# The title is the dedup key - a persisting condition updates one rolling issue instead of opening
# a new one each run - so both the regression alert ('Benchmark regressions detected') and the
# workflow-failure alert route through here with their own title/label. `label` is the
# comma-separated label list applied on creation. Needs GH_TOKEN in the environment (the workflow
# supplies it).
[group('automation')]
[script]
gh-file-rolling-issue title label body_file:
    Set-StrictMode -Version Latest
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

# Posts (or updates in place) the single rolling benchmark comment on a pull request, from a
# pre-rendered body file. Thin wrapper over Publish-RollingComment in
# scripts/bench-history/BenchHistoryComment.psm1 (Pester-tested via `just test-scripts`), the PR
# counterpart of gh-file-rolling-issue: where main keeps a rolling ISSUE keyed by title, a PR keeps
# a rolling COMMENT keyed by a hidden HTML `marker` embedded in the body, so every push updates one
# comment instead of spamming the thread. `gh` reads the body from any path (--body-file semantics),
# so it can live in the runner temp dir, outside the repo checkout. `repo` is `owner/name`
# ('{{repo}}') and `pr` the PR number; both are validated in the module before being spliced into
# the REST path. Needs GH_TOKEN in the environment (the workflow supplies it).
[group('automation')]
[script]
gh-file-pr-comment repo pr marker body_file:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true
    Import-Module ./scripts/bench-history/BenchHistoryComment.psm1 -Force

    Write-Verbose "Posting or updating the rolling benchmark comment on {{repo}} PR #{{pr}} from body file {{body_file}}." -Verbose
    $url = Publish-RollingComment `
        -Repo '{{repo}}' `
        -PrNumber '{{pr}}' `
        -Marker '{{marker}}' `
        -BodyFile '{{body_file}}' `
        -Verbose
    Write-Verbose "Rolling PR comment is available at $url." -Verbose

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
    Set-StrictMode -Version Latest
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
# short-lived OIDC token. release-plz is idempotent - it re-checks the registry and skips
# already-published versions - so a retry (or a whole re-run) safely resumes a partially
# published release, and each attempt mints a fresh OIDC token. NOT for local use: it
# performs real publishes. See docs/release-automation.md.
[group('automation')]
[script]
gh-release config:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true
    Import-Module ./scripts/release/ReleaseAutomation.psm1 -Force

    Invoke-ReleasePublish -ConfigPath '{{config}}'

# Reconciles the desired prebuilt-binary archives against what is already uploaded to each
# published binary crate's GitHub release, and emits the matrix of missing (crate, target)
# pairs still to build as the `matrix` / `has_binaries` step outputs (also printed). This is
# what makes retries self-healing: a re-run rebuilds only whatever is still missing, with no
# hardcoded crate list. Safe to run locally - with no releases yet it reports an empty
# matrix. See .github/workflows/release.yml and docs/release-automation.md.
[group('automation')]
[script]
gh-plan-release-binaries:
    Set-StrictMode -Version Latest
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
