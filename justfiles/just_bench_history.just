# Folo's nightly backfill retains its repository-specific rolling window and shared
# collection settings. History and PR orchestration belong to the reusable workflows.
# Ref: .github/workflows/design.md#benchmark-history.

# An empty eligible range is an explicit successful no-op; existing points are preserved.
[group('automation')]
[script]
gh-backfill-bench-history:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true
    Import-Module ./scripts/bench-history/BenchHistoryCollect.psm1 -Force

    $env:RUSTFLAGS = Get-BenchHistoryRustFlag -Existing $env:RUSTFLAGS -Stability $env:BENCH_STABILITY_RUSTFLAGS
    $toolArgs = @(Get-BenchHistoryBackfillCommand -ToCommitId $env:BACKFILL_TO_COMMIT -Verbose)
    if ($toolArgs.Count -eq 0) {
        Write-Verbose 'No commit is old enough to backfill yet; nothing will be measured.' -Verbose
        return
    }
    cargo run -p cargo-bench-history --bin cargo-bench-history --locked -- @toolArgs
