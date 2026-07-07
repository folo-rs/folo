#requires -Version 7

# Rolling-issue filing for the benchmark-history workflow (.github/workflows/bench-history.yml).
#
# Replaces the former JasonEtco/create-an-issue action for BOTH issues this workflow files (the
# regression alert and the workflow-failure alert). Filing "one issue with a fixed title,
# updated in place when it already exists" is a short `gh` sequence, and hand-rolling it both drops
# a third-party dependency AND lets the rendered body live anywhere `gh` can read it (--body-file
# takes any path), so no scratch file has to sit in the repo checkout where `analyze`'s
# `git status --porcelain` dirty-check would see it. Both callers reach this one seam: the
# regression path via the thin gh-file-rolling-issue `just` recipe (its job already has `just`),
# the lightweight failure-alert job by importing this module directly (it skips the build-env
# setup). Keeping the find-or-file logic here is what lets the Pester suite
# (BenchHistoryIssue.Tests.ps1) exercise it against a mocked `gh` rather than only via a push to
# `main`. The one real GitHub-touching tool (`gh`) is isolated behind small seams the tests mock.

Set-StrictMode -Version Latest

function Invoke-GhCapture {
    # Runs `gh` with the given arguments, capturing stdout and stderr SEPARATELY. stderr is
    # redirected to a temp file so it never contaminates stdout: `gh` can print warnings — e.g.
    # deprecation or rate-limit notes — to stderr while still exiting 0, and folding those into
    # stdout (a bare `2>&1`) would break the JSON/URL parsing the callers do. Returns the captured
    # stdout as a single string on success; on a non-zero exit, throws with whatever `gh` wrote
    # (stderr first, then any stdout) so the failure is never swallowed. Inspecting the exit code
    # ourselves — rather than letting a non-zero `gh` abort — is why the native-error toggle is off.
    # This is the single seam the Pester suite mocks (via `Mock gh`).
    [CmdletBinding()]
    param([Parameter(Mandatory)][object[]] $Arguments)

    $PSNativeCommandUseErrorActionPreference = $false
    $stderrFile = New-TemporaryFile
    try {
        $stderrPath = $stderrFile.FullName
        $stdout = (gh @Arguments 2>$stderrPath | Out-String)
        $exitCode = $LASTEXITCODE
        if ($exitCode -ne 0) {
            $stderr = Get-Content -LiteralPath $stderrPath -Raw
            $parts = @()
            if ($stderr -and $stderr.Trim()) { $parts += $stderr.Trim() }
            if ($stdout -and $stdout.Trim()) { $parts += $stdout.Trim() }
            throw "gh $($Arguments -join ' ') failed (exit $exitCode): $($parts -join ' ')"
        }
        return $stdout
    }
    finally {
        Remove-Item -LiteralPath $stderrFile.FullName -Force -ErrorAction SilentlyContinue
    }
}

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

    # Ask `gh` for the open issues carrying $Label as JSON; Invoke-GhCapture keeps stderr off
    # stdout so ConvertFrom-Json below always sees clean JSON even if `gh` emitted a warning.
    $output = Invoke-GhCapture -Arguments @(
        'issue', 'list', '--state', 'open', '--label', $Label, '--limit', $Limit, '--json', 'number,title,url'
    )

    # A no-match list is the literal `[]`, which ConvertFrom-Json yields as an empty array; the
    # @() wrapper keeps a single-object result enumerable under strict mode.
    $issues = $output | ConvertFrom-Json
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

    if ($existing) {
        Write-Verbose "Found open issue #$($existing.number) ($($existing.url)); updating its body from '$BodyFile' rather than creating a duplicate."
        Invoke-GhCapture -Arguments @('issue', 'edit', $existing.number, '--body-file', $BodyFile) | Out-Null
        return $existing.url
    }

    Write-Verbose "No open issue titled '$Title' found; creating a new one with labels '$Label' and body from '$BodyFile'."
    $output = Invoke-GhCapture -Arguments @('issue', 'create', '--title', $Title, '--label', $Label, '--body-file', $BodyFile)

    # `gh issue create` prints the new issue's URL on success; extract it (stderr is already kept
    # off stdout by Invoke-GhCapture), falling back to the trimmed output if no URL is present.
    $text = $output.Trim()
    $match = [regex]::Match($text, 'https://\S+')
    if ($match.Success) { return $match.Value }
    return $text
}

function Close-RollingIssue {
    # Closes EVERY open issue whose title equals $Title exactly among those carrying $Label, each
    # with an audit $Comment. The mirror image of Publish-RollingIssue: the workflow's `resolve` job
    # calls this once the pipeline is green again so a fixed run does not leave the rolling
    # failure-alert issue rotting open. Closing ALL matches (not just the first) sweeps any backlog
    # of historical duplicates in one pass — the same list-then-exact-title approach
    # Get-OpenIssueByTitle uses, which sidesteps the search-index lag a `gh search` would hit.
    # Returns the numbers it closed (an empty array when none were open). The real `gh` calls go
    # through Invoke-GhCapture so the Pester suite can mock them.
    [CmdletBinding()]
    [OutputType([object[]])]
    param(
        [Parameter(Mandatory)][string] $Title,
        [Parameter(Mandatory)][string] $Label,
        [Parameter(Mandatory)][string] $Comment,
        [int] $Limit = 100
    )

    # `--limit` defeats the 30-result default so a backlog of historical duplicates all come back.
    $output = Invoke-GhCapture -Arguments @(
        'issue', 'list', '--state', 'open', '--label', $Label, '--limit', $Limit, '--json', 'number,title'
    )

    # A no-match list is the literal `[]`, which ConvertFrom-Json yields as an empty array; the
    # @() wrappers keep a single-object result enumerable and .Count-safe under strict mode.
    $issues = $output | ConvertFrom-Json
    $matching = @(@($issues) | Where-Object { $_.title -eq $Title })

    if ($matching.Count -eq 0) {
        Write-Verbose "No open issue titled '$Title' labelled '$Label' to close; nothing to do."
        return @()
    }

    $closed = foreach ($issue in $matching) {
        Write-Verbose "Closing issue #$($issue.number) ('$Title') because the tracked condition has cleared; leaving comment: $Comment"
        Invoke-GhCapture -Arguments @(
            'issue', 'close', $issue.number, '--reason', 'completed', '--comment', $Comment
        ) | Out-Null
        $issue.number
    }
    return @($closed)
}

Export-ModuleMember -Function Get-OpenIssueByTitle, Publish-RollingIssue, Close-RollingIssue
