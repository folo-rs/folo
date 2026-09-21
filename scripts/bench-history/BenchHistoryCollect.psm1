#requires -Version 7

# Folo's collection policy feeds the reusable history/PR callers and the repository's nightly
# backfill recipe. This module also owns Folo's stable compiler flags and backfill date window;
# generic affected-package, artifact and publication decisions belong to the shared Rust tool.
# Ref: .github/workflows/design.md#benchmark-history.
#
# The nightly backfill (Get-BenchHistoryBackfillCommand) fills gaps in the series belonging to
# whichever machine key the nightly runner draws: the GitHub-hosted runner pool is heterogeneous, so
# consecutive pushed commits land on different hardware and every per-key series is sparse. It walks
# a rolling date window - from the newest first-parent commit at least 24 hours old back to the
# oldest first-parent commit newer than 14 days - and relies on the tool's default skip-existing
# behaviour to measure only the commits this runner's partition is missing. Its scope flags come
# from the same policy the reusable callers use, because a backfilled point must be measured exactly
# like a pushed one to be comparable to it.

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

# Backfill accepts full or abbreviated hexadecimal commit IDs, not arbitrary revision expressions.
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

function Get-BenchHistoryScopeArgument {
    # Encodes backfill's scope and measurement flags from the policy supplied to reusable callers.
    # An optional explicit package list narrows this invocation; otherwise it uses workspace
    # collection with the shared exclusions.
    #
    # `--all-features` ensures Cargo runs benchmark targets guarded by `required-features` and
    # compiles feature-gated code paths into every selected package's benchmarks.
    #
    # Each runner stamps its results with its OWN real hardware fingerprint, so a heterogeneous
    # GitHub runner pool splits into one clean wall-clock series per hardware type instead of one
    # jittery series mixing incomparable machines. The shared repetition policy retains minima
    # to shed one-sided runner jitter - a point taken at a lower best-of would sit
    # systematically higher than its neighbours and manufacture a step change in the series.
    # `--verbose` makes the log spell out the resolved machine key and the fingerprint components
    # behind it, so a key change is debuggable from the log alone.
    [CmdletBinding()]
    [OutputType([string[]])]
    param(
        [Parameter()]
        [AllowNull()]
        [AllowEmptyCollection()]
        [string[]] $Package
    )

    $policy = Get-BenchHistoryCollectionPolicy
    $packages = @($Package | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    if ($packages.Count -gt 0) {
        # Explicit backfill scoping retains the caller's selected package order.
        $selection = @()
        foreach ($name in $packages) { $selection += @('--package', $name) }
        Write-Verbose ("Scoping backfill to the explicitly selected packages: " +
            ($packages -join ', ') + '.')
    } else {
        $selection = @('--workspace')
        foreach ($name in $policy.ExcludedPackages) { $selection += @('--exclude', $name) }
        Write-Verbose ("No explicit package scope: benching the whole workspace except the " +
            'excluded packages: ' + ($script:ExcludedPackages -join ', ') + '.')
    }

    if ($policy.AllFeatures) { $selection += '--all-features' }
    return $selection + @(
        '--best-of', [string] $policy.BestOf,
        '--verbose'
    )
}

function Get-BenchHistoryRustFlag {
    # Every collection recipe uses the same alignment policy without dropping unrelated flags.
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
    # toggle is off. This is the single seam the Pester suite mocks (via `Mock git`), so the window
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
                'later by the backfill step, which fails if the ref cannot be resolved.')
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

function Get-BenchHistoryBackfillCommand {
    # Builds the argument vector the nightly gap-filling run passes to the tool after `--`. Returns a
    # string[] holding a `backfill <from> <to> ...` invocation, or an EMPTY array when no commit is
    # eligible yet (a repository whose whole history is still inside the quarantine) - the caller
    # then skips the tool and the run is a successful no-op.
    #
    # $ToCommitId overrides the computed range end (the workflow_dispatch escape hatch); leave it
    # empty for the scheduled run. The scope flags come from Get-BenchHistoryScopeArgument, the same
    # helper the collect builder uses, because a backfilled point is only comparable to its pushed
    # neighbours if it was measured with the same scope and the same `--best-of`.
    #
    # No `--overwrite`: the tool's default skip-existing behaviour is the entire point, since only
    # the commits this runner's own partition is missing are worth measuring. `--ignore-errors` walks
    # past a commit that fails to build instead of abandoning the rest of the window.
    [CmdletBinding()]
    [OutputType([string[]])]
    param(
        [Parameter()]
        [AllowNull()]
        [AllowEmptyString()]
        [string] $ToCommitId
    )

    $window = Get-BenchHistoryBackfillWindow -ToCommitId $ToCommitId
    if ($null -eq $window) {
        return @()
    }

    Write-Verbose ("Backfilling the first-parent range $($window.From)..$($window.To), measuring " +
        'only the commits this machine partition is missing and walking past any commit that ' +
        'fails to build.')
    return @('backfill', $window.From, $window.To) +
        (Get-BenchHistoryScopeArgument) +
        @('--ignore-errors')
}

Export-ModuleMember -Function Get-BenchHistoryCollectionPolicy, Get-BenchHistoryBackfillCommand, Get-BenchHistoryRustFlag
