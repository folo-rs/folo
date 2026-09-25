#requires -Version 7
# The report job in deep-validation.yml publishes readable failures from its own workflow.
# It uses the runner's PowerShell/gh rather than Rust so setup failures remain reportable.
# GitHub owns report identity; downloaded files are disposable diagnostics.
# Ref: ../../.github/workflows/implementation.md#failure-reporting.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
Import-Module (Join-Path $PSScriptRoot 'ScheduledReport.psm1')
Import-Module (Join-Path $PSScriptRoot '..\utility\Retry.psm1')

# Shared with title generation so publishing and discovery use the same report-role contract.
# Ref: ../../docs/scheduled-validation.md#run-report-recognition.
$script:ReportTitlePrefix = 'Scheduled validation failed on '

# Result archives include raw tool output as well as summaries. Bound their transfer and disk
# footprint so an ordinary verbose check cannot consume the reporter's available storage.
$script:ArchiveByteLimit = 64MB
# Decode only bounded log prefixes and selected ZIP entries into memory for issue assembly.
# Longer diagnostics remain available through the original job/artifact links.
$script:TextByteLimit = 4MB
# gh failure messages contain status and connection diagnostics, not checker output.
# Keep enough text for classification without allowing stderr to grow without a bound.
$script:ErrorTextLimit = 16KB

function Invoke-ScheduledGitHubJson {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $Endpoint,
        [ValidateSet('GET', 'POST', 'PATCH')][string] $Method = 'GET',
        [hashtable] $Body
    )
    $request = @{ arguments = @('api', $Endpoint, '--method', $Method); body = $Body }
    if ($Method -ne 'GET') { $request.arguments += '--include' }
    $invoke = {
        # Capture native stderr records without mixing successful CLI warnings into JSON.
        $PSNativeCommandUseErrorActionPreference = $false
        $arguments = $request.arguments
        $output = @(if ($null -ne $request.body) {
            $request.body | ConvertTo-Json -Depth 20 -Compress | & gh @arguments --input - 2>&1
        } else { & gh @arguments 2>&1 })
        if ($LASTEXITCODE -ne 0) {
            $text = $output -join "`n"
            $failure = [InvalidOperationException]::new("GitHub API request failed (exit $LASTEXITCODE): $($arguments -join ' '): $text")
            # Only an explicit rate-limit rejection is safe to replay. Transport failures,
            # server errors and malformed success responses may already have committed a write.
            if ($text -match '(?im)^HTTP/\S+ (429)\b' -or
                ($text -match '(?im)^HTTP/\S+ (403)\b' -and
                    $text -match '(?i)rate limit|temporarily blocked from content creation')) {
                $failure.Data['ScheduledThrottleResponse'] = $text
            }
            throw $failure
        }
        $text = ($output | Where-Object { $_ -isnot [Management.Automation.ErrorRecord] }) -join "`n"
        if ($request.arguments -contains '--include') {
            $parts = $text -split '\r?\n\r?\n', 2
            if ($parts.Count -ne 2 -or $parts[0] -cnotmatch '^HTTP/\S+ 2[0-9][0-9]\b') {
                throw 'GitHub write response lacks successful HTTP headers.'
            }
            return $parts[1]
        }
        return $text
    }
    # Match the read-side gh retry settings used by the benchmark-history helpers.
    # Parsing stays outside the retry: malformed successful JSON is not a network fault.
    # Ref: ../../.github/workflows/design.md#transient-fault-handling.
    $response = if ($Method -eq 'GET') {
        Invoke-WithRetry -Attempt 4 -DelaySeconds 3 -BackoffMultiplier 2 -MaxDelaySeconds 30 `
            -RetryOn { param($failure) Test-TransientFailure $failure.Exception.Message } -Action $invoke
    } else { & $invoke }
    return ,($response -join "`n" | ConvertFrom-Json -AsHashtable -NoEnumerate)
}

function Invoke-ScheduledGitHubWrite {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $Endpoint,
        [ValidateSet('POST', 'PATCH')][string] $Method = 'POST',
        [Parameter(Mandatory)][hashtable] $Body,
        [Parameter(Mandatory)][scriptblock] $FindPersisted
    )
    # GitHub recommends serial writes with at least a second between mutations, and a
    # minute/exponential cooldown for secondary limits. Header deadlines are lower bounds.
    # https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api
    # Finite attempts leave persistent throttling visible rather than keeping the job alive.
    $attemptLimit = 3
    for ($attempt = 0; $attempt -lt $attemptLimit; $attempt++) {
        Start-Sleep -Seconds 1
        try {
            return Invoke-ScheduledGitHubJson $Endpoint -Method $Method -Body $Body
        } catch {
            $failure = $_
            $throttled = $failure.Exception.Data['ScheduledThrottleResponse']
            if ($null -ne $throttled) {
                if ($attempt -eq $attemptLimit - 1) { throw }
                $delay = 60 * [Math]::Pow(2, $attempt)
                if ($throttled -match '(?im)^retry-after:\s*(\d+)\s*$') {
                    $delay = [Math]::Max($delay, [double]$Matches[1])
                }
                if ($throttled -match '(?im)^x-ratelimit-remaining:\s*0\s*$' -and
                    $throttled -match '(?im)^x-ratelimit-reset:\s*(\d+)\s*$') {
                    $delay = [Math]::Max($delay, [double]$Matches[1] - ([datetimeoffset](Get-Date)).ToUnixTimeSeconds() + 1)
                }
                Write-Verbose "GitHub rejected $Method $Endpoint for rate limiting; waiting $delay seconds before reconciliation and attempt $($attempt + 2) of $attemptLimit."
                Start-Sleep -Seconds $delay
            }
            # Read after the cooldown, never during it. An absent result permits another POST
            # only for a known rejection, not an ambiguous response or an unavailable lookup.
            try { $persisted = & $FindPersisted }
            catch { throw [InvalidOperationException]::new("Could not reconcile $Method ${Endpoint}: $($_.Exception.Message)", $failure.Exception) }
            if ($null -ne $persisted) { return $persisted }
            if ($null -eq $throttled) { throw $failure }
        }
    }
}

function Get-ScheduledGitHubCollection {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $Endpoint,
        [string] $Property
    )
    $separator = if ($Endpoint.Contains('?')) { '&' } else { '?' }
    # Follow every API page, including large comment/job/artifact collections.
    # GitHub's maximum page size avoids an artificial workflow-size/report-count limit.
    $page = 1
    do {
        $response = Invoke-ScheduledGitHubJson "${Endpoint}${separator}per_page=100&page=$page"
        if ($Property) {
            if (-not $response.ContainsKey($Property)) { throw "GitHub response lacks $Property." }
            $items = @($response[$Property])
        } else { $items = @($response) }
        $items
        $page++
    } while ($items.Count -eq 100)
}

function Test-ScheduledReportIssue {
    [CmdletBinding()]
    param([Parameter(Mandatory)][hashtable] $Issue)

    return -not $Issue.ContainsKey('pull_request') -and $Issue.state -ceq 'open' -and
        ([string]$Issue.title).StartsWith($script:ReportTitlePrefix, [StringComparison]::Ordinal)
}

function Get-ScheduledReportCandidate {
    [CmdletBinding()]
    param([Parameter(Mandatory)][string] $Repository)

    # GitHub title search is not a prefix query. Filter metadata before reading any content.
    # Ref: ../../docs/scheduled-validation.md#run-report-recognition.
    $query = [Uri]::EscapeDataString("repo:$Repository is:issue is:open in:title `"$($script:ReportTitlePrefix.TrimEnd())`"")
    $pageSize = 100 # GitHub's maximum search page size.
    $searchLimit = 1000 # GitHub exposes only the first results up to this search API limit.
    $candidates = [Collections.Generic.List[hashtable]]::new()
    $page = 1
    $expectedTotal = 0
    do {
        $response = Invoke-ScheduledGitHubJson "search/issues?q=$query&sort=created&order=asc&per_page=$pageSize&page=$page"
        if ($response -isnot [hashtable] -or $response['items'] -isnot [array] -or
            $response['incomplete_results'] -isnot [bool] -or
            ($response['total_count'] -isnot [int] -and $response['total_count'] -isnot [long]) -or
            $response.total_count -lt 0) {
            throw 'GitHub report search lacks a valid items, total_count or incomplete_results field.'
        }
        if ($response.incomplete_results -or $response.total_count -gt $searchLimit) {
            throw 'GitHub report search is incomplete or exceeds its accessible result limit.'
        }
        # Changing totals can shift unseen results onto pages already read.
        if ($page -eq 1) { $expectedTotal = $response.total_count }
        elseif ($response.total_count -ne $expectedTotal) {
            throw 'GitHub report search total changed during pagination; discovery is incomplete.'
        }
        if ($response.items.Count -lt [Math]::Min($pageSize, $expectedTotal - ($page - 1) * $pageSize)) {
            throw 'GitHub report search ended a page before supplying its reported results.'
        }
        foreach ($item in $response.items) {
            if (Test-ScheduledReportIssue $item) { $candidates.Add($item) }
        }
        $page++
    } while (($page - 1) * $pageSize -lt $expectedTotal)

    # Finish discovery before acting on results. Search can lag closure or title changes;
    # refresh only prefix-matching candidates, never broaden discovery to other issue kinds.
    foreach ($candidate in @($candidates | Sort-Object number -Unique)) {
        $issue = Invoke-ScheduledGitHubJson "repos/$Repository/issues/$($candidate.number)"
        if (Test-ScheduledReportIssue $issue) { $issue }
    }
}

function Get-ScheduledReport {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $Repository,
        [Parameter(Mandatory)][string] $AttemptUrl
    )
    $issues = @(Get-ScheduledReportCandidate $Repository)
    $linkBoundary = '(?=$|[\s<>)\].,;!?])'
    $attemptPattern = [regex]::Escape($AttemptUrl) + $linkBoundary
    $bodyAttemptPattern = 'https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+/actions/runs/[1-9][0-9]*/attempts/[1-9][0-9]*' + $linkBoundary
    $issues | Where-Object {
        if ([string]$_.body -cmatch $attemptPattern) { return $true }
        # A body identifying another attempt takes precedence over comparison links in comments.
        if ([string]$_.body -cmatch $bodyAttemptPattern) { return $false }
        if ($_.ContainsKey('comments') -and $_.comments -eq 0) { return $false }
        # Human reports may identify the attempt in discussion. An unavailable discussion
        # must fail lookup, not authorize another issue.
        $discussion = @(Get-ScheduledGitHubCollection "repos/$Repository/issues/$($_.number)/comments")
        return @($discussion | Where-Object body -CMatch $attemptPattern).Count -gt 0
    } | Sort-Object number
}

function Get-ScheduledDownloadStartInfo {
    [CmdletBinding()]
    param([Parameter(Mandatory)][string] $Endpoint)

    # Binary responses bypass PowerShell's text pipeline. Bounded stderr capture preserves
    # the actual HTTP/network error needed to decide whether a GET can be retried.
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = (Get-Command gh -CommandType Application | Select-Object -First 1).Source
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    foreach ($argument in @('api', $Endpoint, '--allow-escape-sequences')) { $start.ArgumentList.Add($argument) }
    return $start
}

function Copy-ScheduledLimitedStream {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][IO.Stream] $Source,
        [Parameter(Mandatory)][IO.Stream] $Destination,
        [Parameter(Mandatory)][ValidateRange(1, [long]::MaxValue)][long] $ByteLimit,
        [Threading.Tasks.Task[int]] $ErrorRead,
        [char[]] $ErrorBuffer
    )
    # Count bytes actually read, not HTTP headers or ZIP entry metadata. One extra byte
    # distinguishes a complete response exactly at the limit from a truncated response.
    $buffer = [byte[]]::new([int][Math]::Min(81920L, $ByteLimit)) # .NET's normal copy buffer size.
    $remaining = $ByteLimit
    while ($true) {
        $requested = if ($remaining -eq 0) { 1 } else { [int][Math]::Min($buffer.Length, $remaining) }
        $read = $Source.ReadAsync($buffer, 0, $requested)
        if ($null -ne $ErrorRead) {
            # A full stderr buffer must interrupt a blocked stdout read, not deadlock gh.
            $null = [Threading.Tasks.Task]::WhenAny([Threading.Tasks.Task[]]@($read, $ErrorRead)).GetAwaiter().GetResult()
            if ($ErrorRead.IsCompleted -and $ErrorRead.GetAwaiter().GetResult() -eq $ErrorBuffer.Length) {
                throw "GitHub error output exceeded its limit: $([string]::new($ErrorBuffer, 0, $ErrorBuffer.Length - 1))"
            }
        }
        $count = $read.GetAwaiter().GetResult()
        if ($count -eq 0) { return $false }
        if ($remaining -eq 0) { return $true }
        $Destination.Write($buffer, 0, $count)
        $remaining -= $count
    }
}

function Save-ScheduledGitHubFile {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $Endpoint,
        [Parameter(Mandatory)][string] $Path,
        [ValidateRange(1, [long]::MaxValue)][long] $ByteLimit = $script:ArchiveByteLimit,
        [switch] $AllowPartial
    )
    $request = @{ Endpoint = $Endpoint; Path = $Path; ByteLimit = $ByteLimit; AllowPartial = $AllowPartial }
    # Retry only this idempotent transfer, never archive parsing or issue publication.
    # Ref: ../../.github/workflows/design.md#transient-fault-handling.
    return Invoke-WithRetry -Attempt 4 -DelaySeconds 3 -BackoffMultiplier 2 -MaxDelaySeconds 30 `
        -RetryOn { param($failure) Test-TransientFailure $failure.Exception.Message } `
        -Action { Invoke-ScheduledDownload @request }
}

function Invoke-ScheduledDownload {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $Endpoint,
        [Parameter(Mandatory)][string] $Path,
        [Parameter(Mandatory)][long] $ByteLimit,
        [switch] $AllowPartial
    )
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = Get-ScheduledDownloadStartInfo $Endpoint
    $file = $null
    $started = $false
    # Callers remove complete downloads and allowed text prefixes after reading. Failed
    # transfers are removed here, before an archive can be mistaken for a complete ZIP.
    $keepFile = $false
    try {
        $file = [IO.File]::Create($Path)
        $started = $process.Start()
        if (-not $started) { throw 'Could not start the GitHub download.' }
        $errorBuffer = [char[]]::new($script:ErrorTextLimit + 1)
        $errorRead = $process.StandardError.ReadBlockAsync($errorBuffer, 0, $errorBuffer.Length)
        $truncated = Copy-ScheduledLimitedStream $process.StandardOutput.BaseStream $file $ByteLimit `
            -ErrorRead $errorRead -ErrorBuffer $errorBuffer
        if ($truncated -and -not $process.HasExited) { $process.Kill($true) }
        $errorCount = $errorRead.GetAwaiter().GetResult()
        if ($errorCount -eq $errorBuffer.Length) {
            throw "GitHub error output exceeded its limit: $([string]::new($errorBuffer, 0, $errorBuffer.Length - 1))"
        }
        $process.WaitForExit()
        if ($truncated -and -not $AllowPartial) {
            throw "Download exceeded the $ByteLimit byte limit; the complete artifact is unavailable."
        }
        if (-not $truncated -and $process.ExitCode -ne 0) {
            throw "GitHub download failed for $Endpoint (exit $($process.ExitCode)): $([string]::new($errorBuffer, 0, $errorCount))"
        }
        $keepFile = $true
        return $truncated
    } finally {
        if ($started -and -not $process.HasExited) { $process.Kill($true); $process.WaitForExit() }
        if ($null -ne $file) { $file.Dispose() }
        $process.Dispose()
        if (-not $keepFile) { [IO.File]::Delete($Path) }
    }
}

function Read-ScheduledArtifactText {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $Repository,
        [Parameter(Mandatory)][hashtable] $Artifact,
        [Parameter(Mandatory)][string] $OutputDirectory
    )
    if ($Artifact.expired) { throw 'The artifact has expired.' }
    $path = Join-Path $OutputDirectory "$([long]$Artifact.id).zip"
    try {
        # A partial archive is never opened, even when its first entries look usable.
        $null = Save-ScheduledGitHubFile "repos/$Repository/actions/artifacts/$([long]$Artifact.id)/zip" $path
        $archive = [IO.Compression.ZipFile]::OpenRead($path)
        try {
            # Read only the summary in place; extraction is unnecessary for reporting.
            $entries = @($archive.Entries | Where-Object FullName -CEQ 'summary.md')
            if ($entries.Count -ne 1) { throw 'Artifact must contain one root summary.md entry.' }
            $source = $entries[0].Open()
            $content = [IO.MemoryStream]::new()
            try {
                $truncated = Copy-ScheduledLimitedStream $source $content $script:TextByteLimit
                $text = [Text.Encoding]::UTF8.GetString($content.GetBuffer(), 0, [int]$content.Length).TrimStart([char]0xFEFF)
                if ($truncated) {
                    $text += "`n`nCheck summary truncated at the $script:TextByteLimit byte limit. Remaining diagnostics are unavailable here; see the original artifact linked above."
                }
                return $text
            } finally { $source.Dispose(); $content.Dispose() }
        } finally { $archive.Dispose() }
    } finally { [IO.File]::Delete($path) }
}

function Invoke-ScheduledReporting {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][ValidatePattern('^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$')][string] $Repository,
        [Parameter(Mandatory)][ValidateRange(1, [long]::MaxValue)][long] $RunId,
        [Parameter(Mandatory)][ValidateRange(1, [int]::MaxValue)][int] $RunAttempt,
        [Parameter(Mandatory)][string] $OutputDirectory
    )
    $endpoint = "repos/$Repository/actions/runs/$RunId"
    $run = Invoke-ScheduledGitHubJson $endpoint
    if ($run.id -ne $RunId -or $run.run_attempt -ne $RunAttempt -or $run.head_sha -cnotmatch '^[a-f0-9]{40}$') {
        throw 'The run must identify the requested current attempt and tested commit.'
    }

    # Read every attempt so cached dependencies survive reporter-only and failed-job reruns.
    # https://docs.github.com/en/rest/actions/workflow-jobs#list-jobs-for-a-workflow-run
    $jobs = @(Get-ScheduledGitHubCollection "$endpoint/jobs?filter=all" -Property jobs)
    foreach ($job in $jobs) {
        if ($job.run_id -ne $RunId -or $job['run_attempt'] -lt 1) {
            throw 'Job results must identify this run and a positive execution attempt.'
        }
    }
    $jobs = @($jobs | Where-Object run_attempt -LE $RunAttempt |
        Group-Object name -CaseSensitive | ForEach-Object {
            $versions = @($_.Group | Sort-Object run_attempt -Descending)
            $latest = $versions[0]
            # GitHub also copies cached jobs into new attempts with new IDs but unchanged
            # execution timestamps. The original row identifies the artifact-producing attempt.
            if ($latest.status -ceq 'completed' -and $latest['started_at'] -and $latest['completed_at']) {
                $versions | Where-Object {
                    $_.conclusion -ceq $latest.conclusion -and
                        $_['started_at'] -ceq $latest.started_at -and $_['completed_at'] -ceq $latest.completed_at
                } | Sort-Object run_attempt | Select-Object -First 1
            } else { $latest }
        })
    $cancellationReasons = @{}
    $cancellationGaps = [Collections.Generic.List[string]]::new()
    $failedJobs = @(foreach ($job in $jobs) {
        if ($job.name -ceq 'report' -or $job.status -cne 'completed') { continue }
        if ($job.conclusion -cin @('failure', 'timed_out', 'action_required')) { $job }
        elseif ($job.conclusion -ceq 'cancelled') {
            # GitHub encodes an execution-limit termination as cancelled, like an operator
            # cancellation. Only its explicit deadline annotation establishes this failure.
            # Ref: ../../.github/workflows/implementation.md#failure-reporting.
            try {
                $checkPrefix = "https://api.github.com/repos/$Repository/check-runs/"
                if ($job['check_run_url'] -notmatch "^$([regex]::Escape($checkPrefix))([1-9][0-9]*)$") {
                    throw "Cancelled job $($job.id) has a missing or unexpected check-run URL '$($job['check_run_url'])'; cannot classify the cancellation."
                }
                $annotations = @(Get-ScheduledGitHubCollection "repos/$Repository/check-runs/$($Matches[1])/annotations")
                $reasons = @($annotations | Where-Object {
                    $_.annotation_level -ceq 'failure' -and
                        $_.message -cmatch '^The job(?: running on runner .+)? has exceeded the maximum execution time\b'
                } | ForEach-Object { $_.message })
                if ($reasons.Count -gt 0) {
                    $cancellationReasons[$job.id] = $reasons -join "`n"
                    $job
                }
            } catch {
                # Preserve established failures before failing this reporting invocation.
                # An unavailable reason must not silently dismiss an unknown cancellation.
                $cancellationGaps.Add("Cancellation reason unavailable for [$($job.name)]($($job.html_url)) (conclusion: cancelled): $($_.Exception.Message)")
            }
        }
    })
    if ($failedJobs.Count -eq 0) {
        if ($cancellationGaps.Count -gt 0) { throw ($cancellationGaps -join "`n") }
        return 'No failed validation jobs; earlier reports are unchanged.'
    }

    $null = New-Item -ItemType Directory -Path $OutputDirectory -Force
    $OutputDirectory = (Resolve-Path -LiteralPath $OutputDirectory).Path
    $attemptUrl = "https://github.com/$Repository/actions/runs/$RunId/attempts/$RunAttempt"
    $reports = @(Get-ScheduledReport $Repository $attemptUrl)
    $existingText = @()
    if ($reports.Count -gt 0) {
        $report = $reports[0]
        $comments = @(Get-ScheduledGitHubCollection "repos/$Repository/issues/$($report.number)/comments")
        $existingText = @([string]$report.body) + @($comments | ForEach-Object { [string]$_.body })
    }

    $notices = [Collections.Generic.List[string]]::new()
    $notices.AddRange($cancellationGaps)
    $artifacts = @()
    try { $artifacts = @(Get-ScheduledGitHubCollection "$endpoint/artifacts" -Property artifacts) }
    catch { $notices.Add("Result artifact inventory is unavailable: $($_.Exception.Message)") }
    $failures = [Collections.Generic.List[hashtable]]::new()
    foreach ($job in $failedJobs) {
        $diagnostics = [Collections.Generic.List[string]]::new()
        $artifactUrls = [Collections.Generic.List[string]]::new()
        $steps = @($job['steps'] | Where-Object { $null -ne $_ -and $_.conclusion -cnotin @('success', 'skipped') })
        $summary = if ($steps.Count -gt 0) {
            'Unsuccessful steps: ' + (($steps | ForEach-Object { "$($_.name) ($($_.conclusion))" }) -join '; ')
        } else { "Job concluded $($job.conclusion); no failed step was recorded." }
        $diagnostics.Add($summary)
        $logPath = Join-Path $OutputDirectory "job-$([long]$job.id).log"
        try {
            $truncated = Save-ScheduledGitHubFile "repos/$Repository/actions/jobs/$([long]$job.id)/logs" `
                $logPath -ByteLimit $script:TextByteLimit -AllowPartial
            if ($truncated) {
                $diagnostics.Add("Job log truncated at the $script:TextByteLimit byte limit. Remaining diagnostics are unavailable here; full log: $($job.html_url)")
            }
            $excerpt = Get-ScheduledLogExcerpt (Get-Content -LiteralPath $logPath -Raw)
            if ([string]::IsNullOrWhiteSpace($excerpt)) { throw 'The job log is empty.' }
            $diagnostics.Add("Observed job log excerpt:`n$excerpt")
            $errorLine = @($excerpt -split '\r?\n' | Where-Object {
                $_ -match '(?i)##\[error\]|\berror\b|failed|panicked|MISSED|TIMEOUT|timed out|undefined behavior'
            } | Select-Object -First 1)
            if ($errorLine.Count -gt 0) { $summary = $errorLine[0] }
        } catch { $diagnostics.Add("Job logs unavailable: $($_.Exception.Message) Full log: $($job.html_url)") }
        finally { [IO.File]::Delete($logPath) }
        $diagnostics.Add("Job execution attempt: $($job.run_attempt)")
        if ($cancellationReasons.ContainsKey($job.id)) {
            # Keep the platform reason ahead of incidental checker errors in the inventory.
            $summary = $cancellationReasons[$job.id]
            $diagnostics.Add("Execution-limit cancellation: $summary")
            $diagnostics.Add('Job execution was interrupted; available checker diagnostics may be incomplete. Unfinished work has no inferred outcome.')
        }
        $resultArtifacts = @($artifacts | Where-Object name -CEQ "scheduled-result-$RunId-$($job.run_attempt)-$($job.name)")
        foreach ($artifact in $resultArtifacts) {
            $artifactUrls.Add("https://github.com/$Repository/actions/runs/$RunId/artifacts/$([long]$artifact.id)")
            try {
                $text = Read-ScheduledArtifactText $Repository $artifact $OutputDirectory
                if ([string]::IsNullOrWhiteSpace($text)) { throw 'The check summary is empty.' }
                if ($cancellationReasons.ContainsKey($job.id) -and $text -cnotmatch '(?m)^## Final result:') {
                    $diagnostics.Add('No final checker result was recorded in the available check summary.')
                }
                $diagnostics.Add($text)
            } catch { $diagnostics.Add("Check summary unavailable: $($_.Exception.Message)") }
        }
        if ($resultArtifacts.Count -eq 0) {
            $diagnostics.Add('No check-summary artifact identifies this job execution. Setup may have failed before the checker ran.')
        }
        $failures.Add(@{
            name = $job.name; url = $job.html_url; conclusion = $job.conclusion
            summary = $summary; diagnostics = $diagnostics.ToArray(); artifact_urls = $artifactUrls.ToArray()
        })
    }
    $messages = @(Format-ScheduledReport -Run $run -AttemptUrl $attemptUrl `
        -Failures $failures.ToArray() -Notices $notices.ToArray() -ExistingText $existingText)
    for ($index = 0; $index -lt $messages.Count; $index++) {
        Set-Content -LiteralPath (Join-Path $OutputDirectory "report-$index.md") -Value $messages[$index] -Encoding utf8 -NoNewline
    }
    if ($reports.Count -eq 0) {
        $date = ([datetimeoffset]$run.run_started_at).UtcDateTime.ToString('yyyy-MM-dd')
        $report = Invoke-ScheduledGitHubWrite "repos/$Repository/issues" -Body @{
            title = "$script:ReportTitlePrefix$date"; body = $messages[0]
        } -FindPersisted {
            $persisted = Get-ScheduledReport $Repository $attemptUrl | Select-Object -First 1
            if ($null -eq $persisted) {
                Write-Verbose "No open report for $attemptUrl is visible in title search. GitHub indexing may lag a persisted creation; an ambiguous write must not be replayed."
            }
            $persisted
        }
        # A lost response may resolve to an existing human report, not the body we sent.
        $comments = @(Get-ScheduledGitHubCollection "repos/$Repository/issues/$($report.number)/comments")
        $existingText = @([string]$report.body) + @($comments | ForEach-Object { [string]$_.body })
    }
    # Compare ordinary text, without author or ownership markers. This also completes a retry
    # interrupted while adding lengthy diagnostics, and supplements a human-authored report.
    foreach ($message in $messages) {
        if ($message -cnotin $existingText) {
            $null = Invoke-ScheduledGitHubWrite "repos/$Repository/issues/$($report.number)/comments" -Body @{ body = $message } -FindPersisted {
                @(Get-ScheduledGitHubCollection "repos/$Repository/issues/$($report.number)/comments") |
                    Where-Object body -CEQ $message | Select-Object -First 1
            }
            $existingText += $message
        }
    }
    # Exact attempt duplicates need only normal issue reconciliation, not a publication journal.
    foreach ($duplicate in @($reports | Select-Object -Skip 1 | Where-Object state -CEQ 'open')) {
        $explanation = "[Copilot speaking]`n`nDuplicate report for $attemptUrl. Continuing triage in $($report.html_url)."
        $findExplanation = {
            @(Get-ScheduledGitHubCollection "repos/$Repository/issues/$($duplicate.number)/comments") |
                Where-Object body -CEQ $explanation | Select-Object -First 1
        }
        if ($null -eq (& $findExplanation)) {
            $null = Invoke-ScheduledGitHubWrite "repos/$Repository/issues/$($duplicate.number)/comments" -Body @{ body = $explanation } -FindPersisted $findExplanation
        }
        $null = Invoke-ScheduledGitHubWrite "repos/$Repository/issues/$($duplicate.number)" -Method PATCH -Body @{ state = 'closed'; state_reason = 'not_planned' } -FindPersisted {
            $observed = Invoke-ScheduledGitHubJson "repos/$Repository/issues/$($duplicate.number)"
            if ($observed.state -ceq 'closed') { $observed }
        }
    }
    if ($cancellationGaps.Count -gt 0) {
        throw "Report: $($report.html_url). Cancellation classification remains incomplete: $($cancellationGaps -join "`n")"
    }
    return "Report: $($report.html_url)"
}

Export-ModuleMember -Function Invoke-ScheduledReporting
