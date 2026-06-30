# Recipes intended ONLY for automation (GitHub Actions), not local use. Each is
# prefixed `gh-` to signal that. They assume the workspace's committed
# `.cargo/bench_history.toml` already points at the prod Azure history store
# (account folohistory) and that the runner is signed in to Azure. We may factor
# these out into dedicated GitHub Actions later (see folo-rs/folo#284), so keep them
# thin wrappers over the tool, free of local-developer conveniences.

# Collects the whole workspace's benchmark history (excluding the slow, special-purpose
# `benchmarks` package) into the prod Azure store, keyed by the current commit. The
# machine partition for noisy wall-clock engines is auto-detected; deterministic engines
# (Callgrind, allocation/time counters) are hardware-independent. `--overwrite` makes a
# re-run on an unchanged commit idempotent.
[group('automation')]
gh-collect-bench-history:
    cargo run -p cargo-bench-history --bin cargo-bench-history --locked -- run --workspace --exclude benchmarks --overwrite --verbose

# Analyzes the collected history for regressions across ALL engines, target triples and
# machine keys, writing two working-directory artefacts: bench-history-report.md (the
# Markdown report) and bench-history-notable.txt (`true`/`false`, the JSON `notable`
# flag). Findings never fail the recipe (the tool always exits 0); the workflow files a
# rolling issue when notable. `auto` mode on a clean `main` checkout resolves to
# long-range history-trend detection. A single analyze pass emits both the Markdown and
# JSON reports via the per-format output toggles (`--no-text --markdown --json`).
[group('automation')]
[script]
gh-analyze-bench-history:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    $reportPath = Join-Path (Get-Location) "bench-history-report.md"
    $jsonPath = Join-Path (Get-Location) "bench-history-report.json"
    $notablePath = Join-Path (Get-Location) "bench-history-notable.txt"
    $common = @('--engine', 'all', '--target-triple', 'all', '--machine-key', 'all')

    Write-Verbose "Analyzing benchmark history in a single pass; writing Markdown to $reportPath and JSON to $jsonPath." -Verbose
    cargo run -p cargo-bench-history --bin cargo-bench-history --locked -- analyze @common --no-text --markdown $reportPath --json $jsonPath
    Write-Verbose "Computing the notable flag from the JSON report." -Verbose
    $json = Get-Content -Path $jsonPath -Raw | ConvertFrom-Json
    $notable = if ($json.notable) { 'true' } else { 'false' }
    Set-Content -Path $notablePath -Value $notable -Encoding utf8
    Write-Verbose "Analysis complete: notable=$notable (report at $reportPath)." -Verbose
