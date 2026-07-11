#requires -Version 7

# Argument selection for the benchmark-history `collect` step (.github/workflows/bench-history.yml,
# via the gh-collect-bench-history recipe).
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

Set-StrictMode -Version Latest

function Get-BenchHistoryCollectCommand {
    # Builds the argument vector passed to the tool after `--` (a `collect ...` or `backfill ...`
    # invocation), choosing the mode from $RecollectCommitId. Returns a string[]; throws when a
    # non-empty id is not a plausible commit SHA. Emits an explanatory verbose note describing which
    # mode was chosen and why, for the workflow log.
    [CmdletBinding()]
    [OutputType([string[]])]
    param(
        [Parameter()]
        [AllowNull()]
        [AllowEmptyString()]
        [string] $RecollectCommitId
    )

    # Scope + noise-reduction flags shared by both modes: `collect` and `backfill` flatten the same
    # clap arg groups, so this array applies verbatim to either subcommand. The fixed `github`
    # machine key keeps the whole GitHub runner pool on one wall-clock series; `--best-of 3` keeps
    # each metric's minimum across three runs to shed one-sided runner jitter.
    $scope = @(
        '--workspace',
        '--exclude', 'benchmarks',
        '--machine-key', 'github',
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
        throw ("Recollect commit id must be a 7-40 character hex commit SHA on main's history, " +
            "got '$recollect'.")
    }

    Write-Verbose ("Recollect commit ${recollect}: re-measuring that single commit in a throwaway " +
        'worktree and overwriting its stored point with `backfill --overwrite`, using this HEAD ' +
        'build of the tool so only the benchmark code - not the collection logic - comes from that ' +
        'commit.')
    return @('backfill', $recollect, $recollect) + $scope + @('--overwrite')
}

Export-ModuleMember -Function Get-BenchHistoryCollectCommand
