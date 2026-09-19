#requires -Version 7

# Argument selection for the benchmark-history collection steps: the push-to-main workflow
# (.github/workflows/bench-history.yml, via the gh-collect-bench-history recipe), the per-PR
# workflow (.github/workflows/pr-bench-history.yml, via gh-collect-pr-bench-history) and the nightly
# gap-filling workflow (.github/workflows/bench-history-backfill.yml, via
# gh-backfill-bench-history).
#
# Collection appends the pushed commit with `collect --skip-existing`, so a re-triggered run
# leaves stored objects unchanged. Manual pruning and ordinary backfill cover data maintenance;
# no targeted repair workflow is needed. Ref: .github/workflows/design.md#benchmark-history.
#
# With no explicit package list the whole workspace is
# benched except the excluded packages (the push-to-main default); the PR workflow
# instead passes the delta-affected packages so it benches only what the PR impacts. Every selected
# package is benched with all Cargo features enabled, so Cargo includes targets guarded by
# `required-features` and builds each package in its all-features configuration.
# Select-BenchmarkablePackage is the shared helper that drops exclusions from a delta-affected set
# before both the scope decision and the "is there anything to bench at all" gate.
#
# The nightly backfill (Get-BenchHistoryBackfillCommand) fills gaps in the series belonging to
# whichever machine key the nightly runner draws: the GitHub-hosted runner pool is heterogeneous, so
# consecutive pushed commits land on different hardware and every per-key series is sparse. It walks
# a rolling date window - from the newest first-parent commit at least 24 hours old back to the
# oldest first-parent commit newer than 14 days - and relies on the tool's default skip-existing
# behaviour to measure only the commits this runner's partition is missing. Its scope flags come
# from the same helper the collect builders use, because a backfilled point must be measured exactly
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

function Select-BenchmarkablePackage {
    # Filters a delta-affected package list down to the ones the benchmark-history workflow actually
    # collects, i.e. everything except the excluded packages. The PR workflow's `delta`
    # job feeds the result into Get-DeltaOutput, so an empty result is what makes the workflow treat
    # "only non-benchmarkable packages changed" as "nothing to bench" (skip collection, clean up any
    # stale comment). Order-preserving; a case-sensitive match, matching how `cargo`/the tool treat
    # package names. Pure, so the filtering is unit-tested independently of the delta orchestration.
    [CmdletBinding()]
    [OutputType([string[]])]
    param(
        [Parameter(Mandatory)]
        [AllowEmptyCollection()]
        [string[]] $Package
    )

    return @($Package | Where-Object { $_ -cnotin $script:ExcludedPackages })
}

function Get-BenchHistoryScopeArgument {
    # Builds the scope + feature-selection + noise-reduction flags EVERY benchmark-history run
    # shares, so a `collect` and a `backfill` can never measure the same commit differently.
    # `collect` and `backfill` flatten the same clap arg groups, so this array applies verbatim to
    # either subcommand.
    #
    # $Package selects the collection scope. When empty (the push-to-main and nightly-backfill
    # default), the whole workspace is benched except the excluded packages (`--workspace` with
    # repeated `--exclude`). When non-empty (the PR workflow, which passes the delta-affected
    # packages), the run is scoped to exactly those packages (`--package <name>` each); the caller is
    # expected to have already applied Select-BenchmarkablePackage.
    #
    # `--all-features` ensures Cargo runs benchmark targets guarded by `required-features` and
    # compiles feature-gated code paths into every selected package's benchmarks.
    #
    # Each runner stamps its results with its OWN real hardware fingerprint, so a heterogeneous
    # GitHub runner pool splits into one clean wall-clock series per hardware type instead of one
    # jittery series mixing incomparable machines. `--best-of 3` keeps each metric's minimum across
    # three runs to shed one-sided runner jitter - a point taken at a lower best-of would sit
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

    $packages = @($Package | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    if ($packages.Count -gt 0) {
        # Explicit package scoping (PR workflow): one `--package <name>` per impacted crate.
        $selection = @()
        foreach ($name in $packages) { $selection += @('--package', $name) }
        Write-Verbose ("Scoping collection to the delta-affected packages: " +
            ($packages -join ', ') + '.')
    } else {
        $selection = @('--workspace')
        foreach ($name in $script:ExcludedPackages) { $selection += @('--exclude', $name) }
        Write-Verbose ("No explicit package scope: benching the whole workspace except the " +
            'excluded packages: ' + ($script:ExcludedPackages -join ', ') + '.')
    }

    return $selection + @(
        '--all-features',
        '--best-of', '3',
        '--verbose'
    )
}

function Get-BenchHistoryCollectCommand {
    # Builds the append-only collection argument vector passed to the tool after `--`.
    # Native argument passing preserves repository/configuration paths as data.
    #
    # $Package selects the collection scope and is handed to Get-BenchHistoryScopeArgument. An empty
    # scope is not an error here: the PR workflow structurally never reaches collection with an empty
    # benchmarkable set (its `delta` job gates that case out to the cleanup path), so the only caller
    # that passes no packages is the push-to-main path, which wants exactly the whole-workspace
    # default.
    [CmdletBinding()]
    [OutputType([string[]])]
    param(
        [Parameter()]
        [AllowNull()]
        [AllowEmptyCollection()]
        [string[]] $Package,

        [AllowNull()][AllowEmptyString()][string] $Repository,
        [AllowNull()][AllowEmptyString()][string] $ConfigPath
    )

    $scope = Get-BenchHistoryScopeArgument -Package $Package
    foreach ($binding in @(
            @{ Parameter = 'Repository'; Flag = '--repo'; Value = $Repository }
            @{ Parameter = 'ConfigPath'; Flag = '--config'; Value = $ConfigPath }
        )) {
        if ($PSBoundParameters.ContainsKey($binding.Parameter)) {
            if ([string]::IsNullOrWhiteSpace($binding.Value)) {
                throw "An explicit $($binding.Parameter) must not be empty."
            }
            $scope += @($binding.Flag, $binding.Value)
        }
    }

    Write-Verbose ('Appending with `collect --skip-existing`, so an already-stored object ' +
        'is left untouched rather than rewritten.')
    return @('collect') + $scope + @('--skip-existing')
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

Export-ModuleMember -Function Get-BenchHistoryCollectCommand, Get-BenchHistoryBackfillCommand, Select-BenchmarkablePackage, Get-BenchHistoryRustFlag
