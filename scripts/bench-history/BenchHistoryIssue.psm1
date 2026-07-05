#requires -Version 7

# Rolling-issue filing for the benchmark-history workflow (.github/workflows/bench-history.yml).
#
# Replaces the former JasonEtco/create-an-issue action. Filing "one issue with a fixed title,
# updated in place when it already exists" is a short `gh` sequence, and hand-rolling it both drops
# a third-party dependency AND lets the rendered body live anywhere `gh` can read it (--body-file
# takes any path), so no scratch file has to sit in the repo checkout where `analyze`'s
# `git status --porcelain` dirty-check would see it. The workflow step is a thin `just` wrapper
# (justfiles/just_automation.just: gh-file-bench-history-issue) that imports this module and calls
# Publish-RollingIssue, so the find-or-file logic lives here where the Pester suite
# (BenchHistoryIssue.Tests.ps1) exercises it against a mocked `gh` rather than only via a push to
# `main`. The one real GitHub-touching tool (`gh`) is isolated behind small seams the tests mock.

Set-StrictMode -Version 3.0

function Get-OpenIssueByTitle {
    # Returns the first OPEN issue whose title equals $Title exactly, or $null when none matches.
    # The list is narrowed to $Label so only a handful of issues come back, then the exact-title
    # match is done client-side — the same list-then-match approach the workflow's `resolve` job
    # uses to find the failure-alert issue, which avoids the eventual-consistency lag of the GitHub
    # search index that a `gh issue list --search`/`gh search issues` query would hit. Isolates the
    # real `gh issue list` call so the tests can mock it.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $Title,
        [Parameter(Mandatory)][string] $Label,
        [int] $Limit = 100
    )

    # Inspect the exit code ourselves rather than letting a non-zero `gh` abort the function; 2>&1
    # merges stderr (where `gh` prints its errors) into the captured output for the message.
    $PSNativeCommandUseErrorActionPreference = $false
    $output = gh issue list --state open --label $Label --limit $Limit --json number,title,url 2>&1
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw "gh issue list (label '$Label') failed (exit $exitCode): $(($output | Out-String).Trim())"
    }

    # A no-match list is the literal `[]`, which ConvertFrom-Json yields as an empty array; the
    # @() wrapper keeps a single-object result enumerable under strict mode.
    $issues = ($output | Out-String) | ConvertFrom-Json
    foreach ($issue in @($issues)) {
        if ($issue.title -eq $Title) { return $issue }
    }
    return $null
}

function Publish-RollingIssue {
    # Files exactly ONE rolling issue: when an open issue with the exact $Title already exists its
    # body is updated in place (so a persisting condition never spams duplicates), otherwise a new
    # issue is created with $Label. The body is read by `gh` from $BodyFile, which may be any path
    # (for example the runner temp dir) — this is what frees the workflow from writing scratch files
    # into the repo checkout. $Label is the comma-separated label list applied on creation; the
    # dedup search is narrowed to the first of those labels. Returns the issue URL.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $Title,
        [Parameter(Mandatory)][string] $Label,
        [Parameter(Mandatory)][string] $BodyFile
    )

    if (-not (Test-Path -LiteralPath $BodyFile)) {
        throw "Issue body file '$BodyFile' does not exist."
    }

    $searchLabel = ($Label -split ',')[0].Trim()
    Write-Verbose "Searching for an existing open issue titled '$Title' among issues labelled '$searchLabel' before filing, so a regression that persists across runs updates one rolling issue instead of opening a duplicate every run."
    $existing = Get-OpenIssueByTitle -Title $Title -Label $searchLabel

    # As above: classify `gh`'s exit code ourselves and surface its output on failure.
    $PSNativeCommandUseErrorActionPreference = $false
    if ($existing) {
        Write-Verbose "Found open issue #$($existing.number) ($($existing.url)); updating its body from '$BodyFile' rather than creating a duplicate."
        $output = gh issue edit $existing.number --body-file $BodyFile 2>&1
        $exitCode = $LASTEXITCODE
        if ($exitCode -ne 0) {
            throw "gh issue edit #$($existing.number) failed (exit $exitCode): $(($output | Out-String).Trim())"
        }
        return $existing.url
    }

    Write-Verbose "No open issue titled '$Title' found; creating a new one with labels '$Label' and body from '$BodyFile'."
    $output = gh issue create --title $Title --label $Label --body-file $BodyFile 2>&1
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw "gh issue create failed (exit $exitCode): $(($output | Out-String).Trim())"
    }

    # `gh issue create` prints the new issue's URL on success; extract it even if a stderr warning
    # was merged in by 2>&1, falling back to the trimmed output if no URL is present.
    $text = ($output | Out-String).Trim()
    $match = [regex]::Match($text, 'https://\S+')
    if ($match.Success) { return $match.Value }
    return $text
}

Export-ModuleMember -Function Get-OpenIssueByTitle, Publish-RollingIssue
