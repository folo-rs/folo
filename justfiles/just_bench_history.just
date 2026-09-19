# Monorepo benchmark workflow entry points. Rust owns report/evidence decisions and GitHub
# publication; these recipes only assemble native invocations through the existing collection,
# path and machine-key helpers. Ref: .github/workflows/design.md#benchmark-history.
#
# TODO(#284): Retire these CI-only recipes when the history.yml, pr.yml and backfill.yml
# reusable workflows in folo-rs/cargo-bench-history-action are published and adopted.
# The root action owns collect, backfill and analyze-history/analyze-pr invocations; the
# reusable workflows own collection receipts and machine-key handoff. Folo's configuration
# and .github/actions/bench-history-setup remain repository-owned.

# Main collection leaves stored measurements unchanged. Manual pruning and ordinary backfill
# cover data maintenance; the workflow has no targeted historical repair mode.
[group('automation')]
[script]
gh-collect-bench-history:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true
    Import-Module ./scripts/bench-history/BenchHistoryCollect.psm1 -Force

    $env:RUSTFLAGS = Get-BenchHistoryRustFlag -Existing $env:RUSTFLAGS -Stability $env:BENCH_STABILITY_RUSTFLAGS
    $toolArgs = Get-BenchHistoryCollectCommand -Verbose
    cargo run -p cargo-bench-history --bin cargo-bench-history --locked -- @toolArgs

# PR automation is built from the event's automation checkout, while --repo points at the
# separately checked-out real PR head. The automation configuration selects the shared store;
# collection preserves existing measurements. CSV package inputs remain data, not PowerShell source.
[group('automation')]
[script]
gh-collect-pr-bench-history $PACKAGES $REPOSITORY:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true
    Import-Module ./scripts/bench-history/BenchHistoryCollect.psm1 -Force

    $packageList = @($env:PACKAGES.Split(',', [StringSplitOptions]::TrimEntries -bor [StringSplitOptions]::RemoveEmptyEntries))
    if ($packageList.Count -eq 0) {
        throw 'PR collection requires a nonempty package scope; refusing a whole-workspace fallback.'
    }
    $env:RUSTFLAGS = Get-BenchHistoryRustFlag -Existing $env:RUSTFLAGS -Stability $env:BENCH_STABILITY_RUSTFLAGS
    $config = Join-Path (Get-Location).Path '.cargo/bench_history.toml'
    $toolArgs = Get-BenchHistoryCollectCommand -Package $packageList -Repository $env:REPOSITORY -ConfigPath $config -Verbose
    cargo run -p cargo-bench-history --bin cargo-bench-history --locked -- @toolArgs

# Nightly densification retains the existing quarantine/window, exclusions, all-features and
# best-of settings. An empty eligible range is a deliberate successful no-op.
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

# Capture the real runner fingerprint only after successful collection. The companion receipt
# command validates and binds this value to the run, attempt, frozen head and platform.
[group('automation')]
[script]
gh-write-bench-history-machine-key $FILE:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true
    Import-Module ./scripts/bench-history/BenchHistoryPath.psm1 -Force

    New-BenchHistoryDirectory -Path (Split-Path -Parent $env:FILE)
    $key = (cargo run -p cargo-bench-history --bin cargo-bench-history --locked -- machine-key --verbose | Select-Object -Last 1)
    $key = if ($null -eq $key) { '' } else { $key.Trim() }
    if ($key -notmatch '^[0-9a-fA-F]{16}$') {
        throw "machine-key produced '$key', not a 16-hex-character fingerprint."
    }
    Set-Content -LiteralPath $env:FILE -Value $key.ToLowerInvariant() -Encoding utf8 -NoNewline

# Both read flows use the same invocation: history passes matching context/base, PRs pass
# their frozen head/base and the real-head repository against the same configured store. The
# companion's inspect-report command emits workflow decisions from the resulting JSON.
[group('automation')]
[script]
gh-analyze-bench-history $DIR $KEYDIR $CONTEXT $BASE $REPOSITORY='':
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true
    Import-Module ./scripts/bench-history/BenchHistoryMachineKey.psm1 -Force
    Import-Module ./scripts/bench-history/BenchHistoryPath.psm1 -Force

    New-BenchHistoryDirectory -Path $env:DIR
    $toolArgs = Get-BenchHistoryAnalysisCommand -KeyDirectory $env:KEYDIR -ReportDirectory $env:DIR -Context $env:CONTEXT -Base $env:BASE -Repository $env:REPOSITORY -Verbose
    cargo run -p cargo-bench-history --bin cargo-bench-history --locked -- @toolArgs
