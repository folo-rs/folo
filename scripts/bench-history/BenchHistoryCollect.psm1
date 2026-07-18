#requires -Version 7

# Argument selection for the benchmark-history `collect` step, shared by the push-to-main workflow
# (.github/workflows/bench-history.yml, via the gh-collect-bench-history recipe) and the per-PR
# workflow (.github/workflows/pr-bench-history.yml, via gh-collect-pr-bench-history).
#
# The step has two modes and the choice between them is real logic - a branch, input validation and
# error handling - so it lives here behind a seam the Pester suite (BenchHistoryCollect.Tests.ps1)
# exercises, and the recipe is a thin import + call. The recollect commit id arrives from an
# untrusted workflow_dispatch input, so validating it here (rather than splicing it into a shell
# command line) is also what keeps it injection-safe.
#
# Normal mode (no recollect id): append the pushed commit with `collect --skip-existing`, so a
# re-triggered run of an already-collected commit is a no-op rather than a rewrite. Recollect mode
# (a commit id set): re-measure just that one historical commit and OVERWRITE its stored point with
# `backfill <id> <id> --overwrite`, which benchmarks the code AT that commit in a throwaway worktree
# while running THIS (HEAD) build of the tool - repairing a point corrupted by a bad benchmark day
# without adopting the tool version that shipped at that commit.
#
# Collection scope is orthogonal to the mode: with no explicit package list the whole workspace is
# benched except the special-purpose `benchmarks` crate (the push-to-main default); the PR workflow
# instead passes the delta-affected packages so it benches only what the PR impacts.
# Select-BenchmarkablePackage is the shared helper that drops `benchmarks` from a delta-affected set
# before both the scope decision and the "is there anything to bench at all" gate.

Set-StrictMode -Version Latest

# The one workspace package the benchmark-history collection never benches: it is the slow,
# special-purpose `benchmarks` crate. The push-to-main workflow excludes it with
# `--workspace --exclude benchmarks`; the PR workflow scopes to the delta-affected packages, so it
# must drop this name from that set before collecting (and before deciding whether anything is left
# to bench at all). Defined once so the exclusion cannot drift between the two paths.
$script:ExcludedPackage = 'benchmarks'

function Select-BenchmarkablePackage {
    # Filters a delta-affected package list down to the ones the benchmark-history workflow actually
    # collects, i.e. everything except the excluded `benchmarks` crate. The PR workflow's `delta`
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

    return @($Package | Where-Object { $_ -cne $script:ExcludedPackage })
}

function Get-BenchHistoryCollectCommand {
    # Builds the argument vector passed to the tool after `--` (a `collect ...` or `backfill ...`
    # invocation), choosing the mode from $RecollectCommitId. Returns a string[]; throws when a
    # non-empty id is not a plausible commit SHA. Emits an explanatory verbose note describing which
    # mode was chosen and why, for the workflow log.
    #
    # $Package selects the collection scope. When empty (the push-to-main default), the whole
    # workspace is benched except the excluded `benchmarks` crate (`--workspace --exclude
    # benchmarks`). When non-empty (the PR workflow, which passes the delta-affected packages), the
    # run is scoped to exactly those packages (`--package <name>` each); the caller is expected to
    # have already dropped `benchmarks` via Select-BenchmarkablePackage. An empty scope is not an
    # error here: the PR workflow structurally never reaches collection with an empty benchmarkable
    # set (its `delta` job gates that case out to the cleanup path), so the only caller that passes
    # no packages is the push-to-main path, which wants exactly the whole-workspace default.
    [CmdletBinding()]
    [OutputType([string[]])]
    param(
        [Parameter()]
        [AllowNull()]
        [AllowEmptyString()]
        [string] $RecollectCommitId,

        [Parameter()]
        [AllowNull()]
        [AllowEmptyCollection()]
        [string[]] $Package
    )

    # Scope + noise-reduction flags shared by both modes: `collect` and `backfill` flatten the same
    # clap arg groups, so this array applies verbatim to either subcommand. No `--machine-key`
    # override: each runner stamps its results with its OWN real hardware fingerprint, so a
    # heterogeneous GitHub runner pool splits into one clean wall-clock series per hardware type
    # instead of one jittery series mixing incomparable machines. `--best-of 3` keeps each metric's
    # minimum across three runs to shed one-sided runner jitter; `--verbose` makes the collect log
    # spell out the resolved machine key and the fingerprint components behind it, so a key change is
    # debuggable from the log alone.
    $packages = @($Package | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    if ($packages.Count -gt 0) {
        # Explicit package scoping (PR workflow): one `--package <name>` per impacted crate.
        $selection = @()
        foreach ($name in $packages) { $selection += @('--package', $name) }
        Write-Verbose ("Scoping collection to the delta-affected packages: " +
            ($packages -join ', ') + '.')
    } else {
        $selection = @('--workspace', '--exclude', $script:ExcludedPackage)
        Write-Verbose ("No explicit package scope: benching the whole workspace except the " +
            "'$script:ExcludedPackage' crate.")
    }

    $scope = $selection + @(
        '--best-of', '3',
        '--verbose'
    )

    $recollect = if ($null -eq $RecollectCommitId) { '' } else { $RecollectCommitId.Trim() }

    if ($recollect -eq '') {
        Write-Verbose ('No recollect commit id: appending the pushed commit with `collect ' +
            '--skip-existing`, so an already-stored object is left untouched rather than rewritten.')
        return @('collect') + $scope + @('--skip-existing')
    }

    # A commit SHA only - hex, 7-40 chars. Rejecting anything else fails a typo'd dispatch loudly
    # (before an expensive benchmark run) and, because the value is an untrusted dispatch input,
    # also guarantees it can carry no shell metacharacters.
    if ($recollect -notmatch '^[0-9a-fA-F]{7,40}$') {
        throw ("Recollect commit id must be a 7-40 character hex commit SHA, got '$recollect'. " +
            "This validates the format only; that the id resolves to a real commit is enforced " +
            "later by the backfill step (which fails if the ref cannot be resolved), while whether " +
            "that commit is actually on main's history is the operator's responsibility - a " +
            'resolvable off-main commit is not rejected.')
    }

    Write-Verbose ("Recollect commit ${recollect}: re-measuring that single commit in a throwaway " +
        'worktree and overwriting its stored point with `backfill --overwrite`, using this HEAD ' +
        'build of the tool so only the benchmark code - not the collection logic - comes from that ' +
        'commit.')
    return @('backfill', $recollect, $recollect) + $scope + @('--overwrite')
}

Export-ModuleMember -Function Get-BenchHistoryCollectCommand, Select-BenchmarkablePackage
