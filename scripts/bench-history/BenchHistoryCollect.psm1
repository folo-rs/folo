#requires -Version 7

# Folo's collection policy feeds the reusable history, PR and backfill callers. This module also
# owns Folo's stable compiler flags and backfill date window, read before Rust setup is available;
# generic affected-package, artifact and publication decisions belong to the shared Rust tool.
# Ref: .github/workflows/design.md#benchmark-history.
#
# The nightly backfill fills gaps for whichever machine key the runner draws. The heterogeneous
# hosted pool leaves each key's series sparse. Folo selects a rolling first-parent window; the
# shared workflow skips commits already measured in this partition. Its measurement inputs come
# from the same policy as the reporting callers so a backfilled point is comparable to a pushed one.

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

# Benchmark-history collection omits the slow, special-purpose `benchmarks` crate and deprecated
# `infinity_pool`. The latter is retained for legacy use, not ongoing performance development;
# excluding it avoids spending CI time and regression-triage effort on performance we do not maintain.
# Main and backfill use Cargo's repeated `--exclude`; PR delta filtering uses the
# same list before deciding whether anything is left to collect. Analysis drops their historical
# series when absent at the context commit; no stored measurements need to be deleted or blessed.
# Ref: .github/workflows/design.md#benchmark-history.
$script:ExcludedPackages = @('benchmarks', 'infinity_pool')

function Get-BenchHistoryCollectionPolicy {
    # Shared caller/backfill settings keep measurements comparable across workflow entry points.
    # Repetitions retain per-metric minima to reduce one-sided hosted-runner noise.
    [CmdletBinding()]
    [OutputType([hashtable])]
    param()

    return @{
        ExcludedPackages = @($script:ExcludedPackages)
        AllFeatures = $true
        BestOf = 3
    }
}

# Folo's manual endpoint accepts full or abbreviated hex commit IDs, not revision expressions.
# Validate explicit range inputs before an expensive benchmark run. The same constraint rejects
# garbled `rev-list` output before it reaches the tool as a range endpoint.
$script:CommitIdPattern = '^[0-9a-fA-F]{7,40}$'

# The nightly backfill's rolling date window, as git approxidate expressions.
#
# The quarantine keeps the newest candidate commit at arm's length from the push-triggered
# collection: a collect job takes hours and can be queued for more, so anything younger risks the
# nightly re-measuring a commit whose own collect run is still in flight and burning a whole run to
# discover a duplicate at the store step.
#
# The horizon bounds how far back the window reaches. Regression detection only ever compares
# against a handful of recent points, so older gaps buy nothing, and the bound also caps how far
# back a point measured with HEAD's benchmark configuration (RUSTFLAGS, collection scope) can be
# planted among neighbours measured with their own.
$script:BackfillQuarantine = '24 hours ago'
$script:BackfillHorizon = '14 days ago'

function Get-BenchHistoryRustFlag {
    # The benchmark setup hook applies the same alignment policy without dropping unrelated flags.
    # The stability value comes from constants.env; this function only replaces prior spellings
    # of that setting so source/toolchain selection does not alter benchmark comparability.
    [CmdletBinding()]
    [OutputType([string])]
    param(
        [AllowNull()][AllowEmptyString()][string] $Existing,
        [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string] $Stability
    )
    $kept = ("$Existing" -replace '(^|\s)(-C\s*|--codegen(?:\s+|=))llvm-args=-align-all-functions=\d+', '').Trim()
    return (@($kept, $Stability) | Where-Object { $_ }) -join ' '
}

function Invoke-GitCapture {
    # Runs `git` with the given arguments and returns its stdout as a string[] of trimmed, non-blank
    # lines. Inspecting the exit code here - rather than letting a non-zero `git` abort the pipeline -
    # is what lets the failure message name the exact query that failed, and is why the native-error
    # toggle is off. This is the boundary the Pester suite mocks (via `Mock git`), so the window
    # resolution below is exercised without a real repository.
    [CmdletBinding()]
    [OutputType([string[]])]
    param(
        [Parameter(Mandatory)][string[]] $Arguments
    )

    $PSNativeCommandUseErrorActionPreference = $false
    $output = @(git @Arguments)
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw "git $($Arguments -join ' ') failed (exit ${exitCode}): $($output -join ' ')"
    }

    return @($output |
            Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
            ForEach-Object { $_.Trim() })
}

function Get-BenchHistoryBackfillWindow {
    # Resolves the rolling date window the nightly backfill walks, as an object carrying `From` and
    # `To` commit ids. Returns $null when nothing is eligible, which the caller reports and treats as
    # a successful no-op run.
    #
    # `To` is the newest first-parent commit at least $script:BackfillQuarantine old, or $ToCommitId
    # when the operator supplied one (the workflow_dispatch escape hatch for stepping over a commit
    # that fails slowly and would otherwise be re-selected every night). `From` is the oldest
    # first-parent commit reachable from `To` that is newer than $script:BackfillHorizon - a horizon
    # measured from NOW, not from `To`, so an override reaching further back than the horizon
    # collapses the window onto that single commit.
    #
    # `From` is resolved FROM `To`, never from HEAD: `backfill` hard-checks that its range start is a
    # first-parent ancestor of its range end and errors out otherwise, and resolving from `To` makes
    # that true by construction. Every query passes `--first-parent` for the same reason - default
    # history simplification can return a commit off the first-parent line, and this repository does
    # carry merge commits.
    [CmdletBinding()]
    [OutputType([pscustomobject])]
    param(
        [Parameter()]
        [AllowNull()]
        [AllowEmptyString()]
        [string] $ToCommitId
    )

    $override = if ($null -eq $ToCommitId) { '' } else { $ToCommitId.Trim() }

    if ($override -eq '') {
        $newest = @(Invoke-GitCapture -Arguments @(
                'rev-list', '-1', '--first-parent', "--before=$script:BackfillQuarantine", 'HEAD'))
        if ($newest.Count -eq 0) {
            Write-Verbose ("No first-parent commit predates '$script:BackfillQuarantine', so every " +
                'commit is still inside the quarantine that keeps this run from racing the ' +
                'push-triggered collection. There is nothing to backfill this run.')
            return $null
        }

        $to = $newest[0]
        Write-Verbose ("Range end $to is the newest first-parent commit predating " +
            "'$script:BackfillQuarantine', so the push-triggered collection of it has long since " +
            'finished and this run cannot race it.')
    } else {
        # Validate the explicit commit ID before using it as a range endpoint.
        if ($override -notmatch $script:CommitIdPattern) {
            throw ("Backfill range end must be a 7-40 character hex commit SHA, got '$override'. " +
                'This validates the format only; that the id resolves to a real commit is enforced ' +
                'by git and the reusable workflow preparation.')
        }

        $to = $override
        Write-Verbose ("Range end $to comes from the operator-supplied override, so the " +
            "'$script:BackfillQuarantine' quarantine is bypassed for this run.")
    }

    $ancestry = @(Invoke-GitCapture -Arguments @(
            'rev-list', '--first-parent', "--since=$script:BackfillHorizon", $to))
    if ($ancestry.Count -eq 0) {
        # `To` itself predates the horizon (a quiet fortnight, or an override reaching further back),
        # so the window collapses onto it. That is a valid single-commit range, unlike the
        # non-ancestor range a HEAD-relative resolution would have produced.
        $from = $to
        Write-Verbose ("No first-parent commit reachable from $to is newer than " +
            "'$script:BackfillHorizon', so the window collapses onto that single commit.")
    } else {
        $from = $ancestry[-1]
        $noun = if ($ancestry.Count -eq 1) { 'commit' } else { 'commits' }
        Write-Verbose ("Range start $from is the oldest of the $($ancestry.Count) first-parent " +
            "$noun reachable from $to and newer than '$script:BackfillHorizon'; older history " +
            'has no comparison value against current tips.')
    }

    foreach ($endpoint in @($from, $to)) {
        if ($endpoint -notmatch $script:CommitIdPattern) {
            throw ("git resolved a backfill range endpoint to '$endpoint', which is not a commit " +
                'SHA. Refusing to hand it to the tool as a range endpoint.')
        }
    }

    return [pscustomobject]@{
        From = $from
        To   = $to
    }
}

Export-ModuleMember -Function Get-BenchHistoryCollectionPolicy, Get-BenchHistoryBackfillWindow, Get-BenchHistoryRustFlag
