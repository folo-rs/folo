#requires -Version 7

# Rolling PR-comment plumbing for the per-PR benchmark-history workflow
# (.github/workflows/pr-bench-history.yml).
#
# The PR counterpart of BenchHistoryIssue.psm1: where the push-to-main workflow keeps one rolling
# ISSUE, the PR workflow keeps one rolling COMMENT on the pull request, updated in place on every
# push so a PR never accumulates a trail of stale benchmark comments. A hidden HTML marker embedded
# in the body is what makes the comment findable again on the next run (mirroring how the issue
# module dedups by exact title). The two callers reach this seam differently: the `analyze` job
# posts findings via the thin gh-file-pr-comment `just` recipe (its job already has `just`), while
# the lightweight `cleanup` job - which runs when a PR no longer touches any benchmarkable package -
# imports this module directly and calls Remove-RollingComment to sweep away a comment left by an
# earlier iteration of the same PR (e.g. a benchmarked change that was later reverted).
#
# Every GitHub-touching call goes through `gh api` behind the single Invoke-GhCapture seam the
# Pester suite (BenchHistoryComment.Tests.ps1) mocks, so the find/update/create/delete logic is
# exercised without touching a real pull request. Unlike an agent-authored GitHub post, this is
# CI/bot output and therefore carries NO `[Copilot speaking]` prefix.

Set-StrictMode -Version Latest

function Invoke-GhCapture {
    # Runs `gh` with the given arguments, capturing stdout and stderr SEPARATELY. stderr is
    # redirected to a temp file so it never contaminates stdout: `gh` can print warnings - e.g.
    # deprecation or rate-limit notes - to stderr while still exiting 0, and folding those into
    # stdout (a bare `2>&1`) would break the JSON parsing the callers do. Returns the captured
    # stdout as a single string on success; on a non-zero exit, throws with whatever `gh` wrote
    # (stderr first, then any stdout) so the failure is never swallowed. Inspecting the exit code
    # ourselves - rather than letting a non-zero `gh` abort - is why the native-error toggle is off.
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

function Assert-RepoAndPr {
    # Guards the two values that get spliced into a `gh api` REST path (`repos/<repo>/issues/...`).
    # Both arrive from workflow context (`github.repository`, the PR number), but validating their
    # shape here keeps the path well-formed and forecloses any path-traversal/injection surprise if
    # a caller ever passes them from a less trustworthy source. Throws on a malformed value.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $Repo,
        [Parameter(Mandatory)][string] $PrNumber
    )

    if ($Repo -notmatch '^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$') {
        throw "Repository must be in 'owner/name' form, got '$Repo'."
    }
    # `^[1-9][0-9]*$` (not `^[0-9]+$`): a PR number is a positive integer, so reject `0` and any
    # leading-zero form to match the error message and keep a well-formed REST path.
    if ($PrNumber -notmatch '^[1-9][0-9]*$') {
        throw "Pull request number must be a positive integer, got '$PrNumber'."
    }
}

function Find-RollingComment {
    # Returns the first issue comment on the pull request whose body contains $Marker, or $null when
    # none does. Issue comments (which is what PR conversation comments are) come back from a single
    # paginated `gh api` call - `--paginate` merges every page into one JSON array, so a PR with more
    # than one page of comments is handled without special-casing. Matching on the hidden marker
    # rather than the author lets the same rolling comment be found regardless of which bot identity
    # posted it. Isolates the real `gh api` list call so the tests can mock it.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $Repo,
        [Parameter(Mandatory)][string] $PrNumber,
        [Parameter(Mandatory)][string] $Marker
    )

    Assert-RepoAndPr -Repo $Repo -PrNumber $PrNumber

    # Invoke-GhCapture keeps stderr off stdout so ConvertFrom-Json always sees clean JSON even if
    # `gh` emitted a warning; `--paginate` follows the next-page links and merges the array pages.
    $output = Invoke-GhCapture -Arguments @(
        'api', '--paginate', "repos/$Repo/issues/$PrNumber/comments"
    )

    # A no-comments list is the literal `[]`, which ConvertFrom-Json yields as an empty array; the
    # @() wrapper keeps a single-object result enumerable under strict mode.
    $comments = $output | ConvertFrom-Json
    # A literal substring match (.Contains, ordinal) rather than -like: the marker is a fixed HTML
    # string, so treating it as a wildcard pattern would misbehave if it ever gained wildcard
    # metacharacters ([, ], *, ?).
    foreach ($comment in @($comments)) {
        if ($comment.body -and $comment.body.Contains($Marker)) { return $comment }
    }
    return $null
}

function Publish-RollingComment {
    # Maintains exactly ONE rolling comment on the pull request: when a comment carrying $Marker
    # already exists its body is PATCHed in place (so repeated pushes update one comment instead of
    # spamming the thread), otherwise a new comment is POSTed. The rendered body is read from
    # $BodyFile (any path - typically the runner temp dir, so no scratch file lands in the checkout
    # where `analyze`'s dirty-check would see it). The hidden $Marker is guaranteed to be present in
    # whatever gets posted - prepended when the rendered body does not already contain it - so the
    # NEXT run can find this same comment. Returns the comment's html_url.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $Repo,
        [Parameter(Mandatory)][string] $PrNumber,
        [Parameter(Mandatory)][string] $Marker,
        [Parameter(Mandatory)][string] $BodyFile
    )

    Assert-RepoAndPr -Repo $Repo -PrNumber $PrNumber

    if (-not (Test-Path -LiteralPath $BodyFile)) {
        throw "Comment body file '$BodyFile' does not exist."
    }
    $body = Get-Content -LiteralPath $BodyFile -Raw
    if ([string]::IsNullOrWhiteSpace($body)) {
        throw "Comment body file '$BodyFile' is empty."
    }

    # Hand the body to `gh` through a FILE (`-F body=@<path>`) rather than inline on the command
    # line: a rendered report can run to tens of kilobytes, which would risk the OS command-line
    # length limit (~32 KB on Windows), force fragile shell-escaping of arbitrary Markdown, and
    # expose the body in process listings. The common case sends $BodyFile untouched. Only when the
    # rendered body lacks the dedup $Marker - this module's responsibility, not the caller's, so the
    # contract holds even if a future body template forgets to embed it - do we materialise a
    # marker-prefixed temp file and send that instead, deleting it once `gh` has run.
    # Literal substring check (.Contains, ordinal) rather than -notlike: the marker is a fixed hidden
    # string, so a wildcard match would misfire if it ever contained wildcard metacharacters.
    $bodyFileToSend = $BodyFile
    $tempBodyFile = $null
    if (-not $body.Contains($Marker)) {
        $tempBodyFile = New-TemporaryFile
        Set-Content -LiteralPath $tempBodyFile.FullName -Value "$Marker`n`n$body" -Encoding utf8 -NoNewline
        $bodyFileToSend = $tempBodyFile.FullName
    }

    try {
        $existing = Find-RollingComment -Repo $Repo -PrNumber $PrNumber -Marker $Marker

        if ($existing) {
            Write-Verbose ("Updating existing rolling comment #$($existing.id) on PR #$PrNumber in place " +
                "rather than posting a duplicate.")
            $output = Invoke-GhCapture -Arguments @(
                'api', '--method', 'PATCH', "repos/$Repo/issues/comments/$($existing.id)", '-F', "body=@$bodyFileToSend"
            )
        } else {
            Write-Verbose ("No rolling comment found on PR #$PrNumber; posting a new one carrying the " +
                "dedup marker so later pushes update it.")
            $output = Invoke-GhCapture -Arguments @(
                'api', '--method', 'POST', "repos/$Repo/issues/$PrNumber/comments", '-F', "body=@$bodyFileToSend"
            )
        }
    }
    finally {
        if ($tempBodyFile) {
            Remove-Item -LiteralPath $tempBodyFile.FullName -Force -ErrorAction SilentlyContinue
        }
    }

    # Both the PATCH and POST comment endpoints return the comment object; surface its html_url,
    # falling back to the existing url (edit path) or empty string if the field is somehow absent.
    $result = $output | ConvertFrom-Json
    if ($result -and $result.html_url) { return $result.html_url }
    if ($existing -and $existing.html_url) { return $existing.html_url }
    return ''
}

function Remove-RollingComment {
    # Deletes the rolling comment (the one carrying $Marker) from the pull request, if present; a
    # no-op that returns $false when none is found. The mirror image of Publish-RollingComment,
    # called by the `cleanup` job when the PR no longer touches any benchmarkable package: a comment
    # left by an earlier iteration (which did touch one) must not linger and mislead. Returns $true
    # when a comment was deleted. The real `gh api` calls go through Invoke-GhCapture so the Pester
    # suite can mock them. SupportsShouldProcess because deleting a GitHub comment is a state-changing
    # action (matches the repo convention for Set-/New-/Stop- helpers): the DELETE is gated on
    # ShouldProcess so `-WhatIf` reports the delete without performing it.
    [CmdletBinding(SupportsShouldProcess)]
    [OutputType([bool])]
    param(
        [Parameter(Mandatory)][string] $Repo,
        [Parameter(Mandatory)][string] $PrNumber,
        [Parameter(Mandatory)][string] $Marker
    )

    Assert-RepoAndPr -Repo $Repo -PrNumber $PrNumber

    $existing = Find-RollingComment -Repo $Repo -PrNumber $PrNumber -Marker $Marker
    if (-not $existing) {
        Write-Verbose ("No rolling comment on PR #$PrNumber to remove; nothing to do (the PR either " +
            "never had one or it was already deleted).")
        return $false
    }

    if (-not $PSCmdlet.ShouldProcess("comment #$($existing.id) on PR #$PrNumber", 'Delete rolling comment')) {
        return $false
    }

    Write-Verbose ("Deleting stale rolling comment #$($existing.id) from PR #$PrNumber because the PR " +
        "no longer touches any benchmarkable package.")
    Invoke-GhCapture -Arguments @(
        'api', '--method', 'DELETE', "repos/$Repo/issues/comments/$($existing.id)"
    ) | Out-Null
    return $true
}

Export-ModuleMember -Function Find-RollingComment, Publish-RollingComment, Remove-RollingComment
