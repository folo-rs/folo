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
# A benchmark run takes many hours, and a new push cancels the in-flight one, so the comment a reader
# sees can lag the PR tip by a long way. Two seams keep that honest: the analyze job embeds a hidden
# "analyzed commit" marker in the body (which commit the numbers describe), and the mark-stale job -
# which fires at the START of a new run - calls Set-RollingCommentStaleness to prepend a warning
# banner stating how far behind HEAD those numbers now are (via Get-CommitsBehind), so nobody mistakes
# hours-old results for the current state. The banner clears itself when the next analyze rewrites the
# body with fresh results.
#
# Every GitHub-touching call goes through `gh api` behind the single Invoke-GhCapture seam the
# Pester suite (BenchHistoryComment.Tests.ps1) mocks, so the find/update/create/delete logic is
# exercised without touching a real pull request. Unlike an agent-authored GitHub post, this is
# CI/bot output and therefore carries NO `[Copilot speaking]` prefix.

Set-StrictMode -Version Latest

# Sentinel pair bounding the staleness warning banner Set-RollingCommentStaleness prepends. Keeping the
# banner between two hidden markers makes a re-run REPLACE it (strip the old block, insert the new)
# rather than stack a second copy, and lets the block be located without depending on its wording.
# Purely internal to this module - only Set-RollingCommentStaleness produces and consumes it - so
# unlike the caller-supplied dedup and analyzed-commit markers it needs no workflow-level definition.
$script:StaleBannerOpen = '<!-- folo-bench-history-stale -->'
$script:StaleBannerClose = '<!-- /folo-bench-history-stale -->'

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
    # `-WhatIf:$false` on the temp-file bookkeeping: New-TemporaryFile and Remove-Item both support
    # ShouldProcess, so an ambient $WhatIfPreference from a SupportsShouldProcess CALLER (e.g.
    # Set-RollingCommentStaleness -WhatIf, which still needs the read GETs that back its preview) would
    # otherwise make New-TemporaryFile a no-op and null out $stderrFile. This stderr redirect is a
    # read-side implementation detail, never the state change -WhatIf is meant to gate, so it must run
    # regardless; the caller gates the actual mutating `gh` call itself.
    $stderrFile = New-TemporaryFile -WhatIf:$false
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
        Remove-Item -LiteralPath $stderrFile.FullName -Force -ErrorAction SilentlyContinue -WhatIf:$false
    }
}

function Assert-Repo {
    # Guards the `owner/name` value spliced into every `gh api` REST path (`repos/<repo>/...`). It
    # arrives from workflow context (`github.repository`), but validating its shape here keeps the path
    # well-formed and forecloses any path-traversal/injection surprise if a caller ever passes it from a
    # less trustworthy source. Throws on a malformed value. Shared by Assert-RepoAndPr and the
    # compare-API caller (which has no PR number).
    [CmdletBinding()]
    param([Parameter(Mandatory)][string] $Repo)

    if ($Repo -notmatch '^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$') {
        throw "Repository must be in 'owner/name' form, got '$Repo'."
    }
}

function Assert-CommitSha {
    # Guards a commit SHA spliced into a `gh api` compare path (`repos/<repo>/compare/<base>...<head>`).
    # Both the PR head SHA (from `github.event.pull_request.head.sha`) and the analyzed SHA parsed from
    # the hidden marker are full 40-char hex, so requiring that shape both rejects a malformed/injected
    # value and keeps the REST path well-formed. $Label names which SHA in the error. Throws on a
    # malformed value.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $Sha,
        [Parameter(Mandatory)][string] $Label
    )

    if ($Sha -notmatch '^[0-9a-fA-F]{40}$') {
        throw "$Label commit SHA must be a 40-character hex string, got '$Sha'."
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

    Assert-Repo -Repo $Repo
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

function Get-CommitsBehind {
    # Counts how many commits $HeadSha carries beyond $BaseSha using the GitHub compare API, so the
    # rolling comment can state how far its analyzed commit lags the current PR tip WITHOUT a
    # full-history clone: the topology is computed server-side and still resolves a commit orphaned by a
    # force-push (which a fresh shallow checkout would not contain). Returns a hashtable:
    #   @{ Related = $true;  Behind = <int> }  when the two commits share history (Behind = `ahead_by`,
    #                                           the commits HEAD has that BASE lacks - exactly the
    #                                           "results are N commits behind the tip" figure);
    #   @{ Related = $false; Behind = 0 }       when they have NO common ancestor (compare 404s) or the
    #                                           base SHA no longer resolves - the caller then renders the
    #                                           numberless "out of date" wording.
    # Only a 404 is treated as "not comparable"; every other `gh` failure is a genuine error and is
    # rethrown, so a transient outage is never silently reported as "out of date". Isolates the real
    # `gh api` compare call behind Invoke-GhCapture so the tests can mock it.
    [CmdletBinding()]
    [OutputType([hashtable])]
    param(
        [Parameter(Mandatory)][string] $Repo,
        [Parameter(Mandatory)][string] $BaseSha,
        [Parameter(Mandatory)][string] $HeadSha
    )

    Assert-Repo -Repo $Repo
    Assert-CommitSha -Sha $BaseSha -Label 'Base'
    Assert-CommitSha -Sha $HeadSha -Label 'Head'

    try {
        # `<base>...<head>` (three dots) is the compare endpoint's basehead syntax; both SHAs are
        # validated 40-hex above, so the path is safe to splice.
        $output = Invoke-GhCapture -Arguments @(
            'api', "repos/$Repo/compare/$BaseSha...$HeadSha"
        )
    }
    catch {
        # The compare endpoint 404s with "No common ancestor for the two commits" for unrelated
        # histories (e.g. a force-push that replaced the branch) and with a not-found message when the
        # base SHA no longer resolves. Both mean "cannot express a distance" -> Related=$false so the
        # caller falls back to the numberless wording. Match the HTTP status (gh appends "(HTTP 404)")
        # or the specific messages; anything else is a real failure and propagates.
        $message = $_.Exception.Message
        if ($message -match 'HTTP 404' -or $message -match '(?i)no common ancestor' -or
            $message -match '(?i)no commit found') {
            return @{ Related = $false; Behind = 0 }
        }
        throw
    }

    $comparison = $output | ConvertFrom-Json
    # `ahead_by` is always present on a successful compare; guard defensively so a schema surprise
    # degrades to the "out of date" wording rather than throwing.
    if ($comparison -and ($comparison.PSObject.Properties.Name -contains 'ahead_by')) {
        return @{ Related = $true; Behind = [int] $comparison.ahead_by }
    }
    return @{ Related = $false; Behind = 0 }
}

function Add-StalenessBanner {
    # Pure string transform: returns $Body with a fresh staleness banner carrying $Warning inserted
    # just after the dedup $Marker line, replacing any banner a previous run left behind. Factored out
    # of Set-RollingCommentStaleness (and kept unexported) so the strip/insert bookkeeping is one
    # testable place. Idempotent by construction: it first removes every line of any existing
    # sentinel-bounded block, then rebuilds "marker, blank, banner, blank, rest" with the rest's leading
    # blank lines trimmed, so re-running on its own output yields byte-identical text (no stacking, no
    # accumulating blank lines).
    [CmdletBinding()]
    [OutputType([string])]
    param(
        [Parameter(Mandatory)][string] $Body,
        [Parameter(Mandatory)][string] $Warning,
        [Parameter(Mandatory)][string] $Marker
    )

    # 1. Drop any previously-inserted banner block (open..close inclusive).
    $kept = [System.Collections.Generic.List[string]]::new()
    $inBanner = $false
    foreach ($line in @($Body -split "`n")) {
        $trimmed = $line.Trim()
        if ($trimmed -eq $script:StaleBannerOpen) { $inBanner = $true; continue }
        if ($inBanner) {
            if ($trimmed -eq $script:StaleBannerClose) { $inBanner = $false }
            continue
        }
        $kept.Add($line)
    }

    # 2. Build the fresh banner: a GitHub "warning" alert bounded by the sentinel pair so the next
    #    strip finds it.
    $banner = @(
        $script:StaleBannerOpen
        '> [!WARNING]'
        "> $Warning"
        $script:StaleBannerClose
    )

    # 3. Insert it right after the dedup marker line, with exactly one blank line on each side. If the
    #    marker is somehow absent, prepend the banner at the very top instead.
    $markerIndex = -1
    for ($i = 0; $i -lt $kept.Count; $i++) {
        if ($kept[$i].Contains($Marker)) { $markerIndex = $i; break }
    }
    if ($markerIndex -lt 0) {
        return (($banner + @('') + @($kept)) -join "`n")
    }

    $after = [System.Collections.Generic.List[string]]::new()
    for ($i = $markerIndex + 1; $i -lt $kept.Count; $i++) { $after.Add($kept[$i]) }
    while ($after.Count -gt 0 -and $after[0].Trim() -eq '') { $after.RemoveAt(0) }

    $assembled = [System.Collections.Generic.List[string]]::new()
    for ($i = 0; $i -le $markerIndex; $i++) { $assembled.Add($kept[$i]) }
    $assembled.Add('')
    foreach ($b in $banner) { $assembled.Add($b) }
    $assembled.Add('')
    foreach ($a in $after) { $assembled.Add($a) }
    return ($assembled -join "`n")
}

function Set-RollingCommentStaleness {
    # Called at the START of a new PR benchmark run (the mark-stale job) to flag that the rolling
    # comment's currently-displayed findings now lag the PR tip and a fresh run is underway, so a reader
    # never mistakes hours-old numbers for the current state. Finds the rolling comment by $Marker
    # (no-op when the PR has none yet); reads the analyzed commit SHA from the hidden
    # "$CommitMarkerPrefix<40-hex> -->" marker the analyze job embeds; asks Get-CommitsBehind how far
    # that commit lags $HeadSha; and PATCHes a warning banner onto the TOP of the comment body. Three
    # outcomes:
    #   * a concrete distance                       -> "... N commit(s) behind HEAD ..."
    #   * unrelated histories / no analyzed marker  -> generic "... out of date ..." (no number)
    #   * already at $HeadSha (distance 0)          -> no banner is warranted, so nothing is patched.
    # The banner is bounded by a sentinel pair (see Add-StalenessBanner) so a re-run replaces rather
    # than stacks it, and the next completed analyze - which rewrites the whole body from scratch -
    # drops it automatically once real new results land. Returns $true when the comment was patched.
    # SupportsShouldProcess because editing a comment is state-changing (matches the module's other
    # mutating helpers): the PATCH is gated on ShouldProcess so `-WhatIf` reports without performing it.
    [CmdletBinding(SupportsShouldProcess)]
    [OutputType([bool])]
    param(
        [Parameter(Mandatory)][string] $Repo,
        [Parameter(Mandatory)][string] $PrNumber,
        [Parameter(Mandatory)][string] $Marker,
        [Parameter(Mandatory)][string] $CommitMarkerPrefix,
        [Parameter(Mandatory)][string] $HeadSha
    )

    Assert-RepoAndPr -Repo $Repo -PrNumber $PrNumber
    Assert-CommitSha -Sha $HeadSha -Label 'Head'

    $existing = Find-RollingComment -Repo $Repo -PrNumber $PrNumber -Marker $Marker
    if (-not $existing) {
        Write-Verbose ("No rolling comment on PR #$PrNumber to flag as stale; nothing to do (the first " +
            "completed run will post one carrying the analyzed-commit marker).")
        return $false
    }

    $body = $existing.body

    # Pull the analyzed commit SHA out of the hidden marker the analyze job embeds
    # ("$CommitMarkerPrefix<40-hex> -->"). A pre-change comment - or any body missing the marker -
    # yields no SHA, in which case a distance cannot be expressed and we fall back to the numberless
    # wording. Escape the prefix: it is HTML and contains regex metacharacters (`!`, `-`).
    $analyzedSha = $null
    $match = [regex]::Match($body, "$([regex]::Escape($CommitMarkerPrefix))([0-9a-fA-F]{40})")
    if ($match.Success) { $analyzedSha = $match.Groups[1].Value }

    $warning = $null
    if ($analyzedSha) {
        $distance = Get-CommitsBehind -Repo $Repo -BaseSha $analyzedSha -HeadSha $HeadSha
        if ($distance.Related -and $distance.Behind -eq 0) {
            Write-Verbose ("Rolling comment on PR #$PrNumber already reflects HEAD ($HeadSha); leaving it " +
                "unmarked.")
            return $false
        }
        if ($distance.Related) {
            $noun = if ($distance.Behind -eq 1) { 'commit' } else { 'commits' }
            $warning = "Benchmark results are $($distance.Behind) $noun behind HEAD. This comment will be updated when newer results are available."
        }
    }
    if (-not $warning) {
        $warning = 'Benchmark results are out of date. This comment will be updated when newer results are available.'
    }

    $newBody = Add-StalenessBanner -Body $body -Warning $warning -Marker $Marker
    if ($newBody -eq $body) {
        Write-Verbose ("Rolling comment on PR #$PrNumber already carries this staleness banner; no update " +
            "needed.")
        return $false
    }

    if (-not $PSCmdlet.ShouldProcess("comment #$($existing.id) on PR #$PrNumber", 'Flag benchmark comment as stale')) {
        return $false
    }

    # Send the edited body through a FILE (`-F body=@<path>`) for the same reasons Publish-RollingComment
    # does: a rendered report can run to tens of kilobytes (over the command-line length limit), it
    # avoids shell-escaping arbitrary Markdown, and it keeps the body out of process listings.
    $tempBodyFile = New-TemporaryFile
    try {
        Set-Content -LiteralPath $tempBodyFile.FullName -Value $newBody -Encoding utf8 -NoNewline
        Write-Verbose ("Flagging rolling comment #$($existing.id) on PR #$PrNumber as stale: $warning")
        Invoke-GhCapture -Arguments @(
            'api', '--method', 'PATCH', "repos/$Repo/issues/comments/$($existing.id)", '-F', "body=@$($tempBodyFile.FullName)"
        ) | Out-Null
    }
    finally {
        Remove-Item -LiteralPath $tempBodyFile.FullName -Force -ErrorAction SilentlyContinue
    }
    return $true
}

Export-ModuleMember -Function Find-RollingComment, Publish-RollingComment, Remove-RollingComment, Get-CommitsBehind, Set-RollingCommentStaleness
