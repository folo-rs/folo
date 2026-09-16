#requires -Version 7
# Formats the report job's observed failures as ordinary Markdown. No issue payload
# schema is required by readers or by human authors. PowerShell is necessary here because a
# Rust setup failure must still be reportable by ScheduledGitHub.psm1.
# Ref: ../../.github/workflows/implementation.md#failure-reporting.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

function Get-ScheduledLogExcerpt {
    [CmdletBinding()]
    param([Parameter(Mandatory)][AllowEmptyString()][string] $Text)

    # Keep every diagnostic match with nearby context, not the first N findings. Noise from
    # successful steps is supplementary in the full logs. The tail explains unfamiliar failures.
    $lines = @($Text -replace '\x1B\[[0-?]*[ -/]*[@-~]', '' -split '\r?\n')
    $selected = [Collections.Generic.SortedSet[int]]::new()
    for ($index = 0; $index -lt $lines.Count; $index++) {
        if ($lines[$index] -match '(?i)##\[error\]|\berror\b|\bfailed\b|\bfailure\b|panicked|MISSED|TIMEOUT|timed out|seed|undefined behavior|uncovered mutant') {
            # Neighboring lines retain test names, source locations and compiler explanations.
            foreach ($neighbor in ([Math]::Max(0, $index - 2)..[Math]::Min($lines.Count - 1, $index + 3))) {
                $null = $selected.Add($neighbor)
            }
        }
    }
    if ($selected.Count -eq 0) {
        foreach ($index in ([Math]::Max(0, $lines.Count - 20)..($lines.Count - 1))) {
            $null = $selected.Add($index)
        }
    }
    $excerpt = [Collections.Generic.List[string]]::new()
    $previous = -1
    foreach ($index in $selected) {
        if ($index -gt $previous + 1) { $excerpt.Add('[Other log lines omitted; full job logs are linked above.]') }
        $excerpt.Add($lines[$index])
        $previous = $index
    }
    if ($previous -lt $lines.Count - 1) {
        $excerpt.Add('[Other log lines omitted; full job logs are linked above.]')
    }
    return ($excerpt -join "`n").Trim()
}

function ConvertTo-ScheduledTableCell {
    [CmdletBinding()]
    param([AllowEmptyString()][string] $Text)
    return [Net.WebUtility]::HtmlEncode($Text).Replace('|', '&#124;').Replace("`r", '').Replace("`n", '<br>')
}

function Get-ScheduledTextExcerpt {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][AllowEmptyString()][string] $Text,
        [Parameter(Mandatory)][int] $Length
    )
    if ($Text.Length -le $Length) { return $Text }
    $notice = "`n[Diagnostic text omitted; see the linked full logs and artifacts.]`n"
    # Keep setup/replay context at the start and final results at the end. The limit includes
    # the notice, and neither boundary may split a UTF-16 surrogate pair.
    $head = [int][Math]::Floor(($Length - $notice.Length) * 0.75)
    $tail = $Text.Length - ($Length - $notice.Length - $head)
    if ([char]::IsHighSurrogate($Text[$head - 1])) { $head-- }
    if ([char]::IsLowSurrogate($Text[$tail])) { $tail++ }
    return $Text.Substring(0, $head) + $notice + $Text.Substring($tail)
}

function Format-ScheduledReport {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][hashtable] $Run,
        [Parameter(Mandatory)][string] $AttemptUrl,
        [Parameter(Mandatory)][hashtable[]] $Failures,
        [string[]] $Notices = @(),
        [string[]] $ExistingText = @()
    )

    # Bound each job independently of log volume; even all-newline text fits after indentation.
    # Pack whole sections so a persisted heading always means that section is complete.
    # Ref: ../../.github/workflows/implementation.md#failure-reporting.
    $messageLimit = 60000
    $diagnosticLimit = 8000
    # Fixed-size inventory pages keep their identity independent of diagnostic availability.
    $jobsPerPage = 25
    $prefix = "[Copilot speaking]`n`nWorkflow attempt: $AttemptUrl"
    $blocks = [Collections.Generic.List[hashtable]]::new()
    $started = ([datetimeoffset]$Run.run_started_at).UtcDateTime.ToString('yyyy-MM-dd HH:mm:ss')
    $metadata = "Workflow: $(ConvertTo-ScheduledTableCell $Run.name)`n`nUTC start: $started`n`nTested commit: ``$($Run.head_sha)```n`nWorkflow status: $($Run.status)`n`n[Workflow logs and result artifacts]($AttemptUrl)"
    if ($Notices.Count -gt 0) {
        $metadata += "`n`n$(Get-ScheduledTextExcerpt ($Notices -join "`n") $diagnosticLimit)"
    }
    for ($offset = 0; $offset -lt $Failures.Count; $offset += $jobsPerPage) {
        $heading = "## Failed jobs (page $([int]($offset / $jobsPerPage) + 1))"
        $table = "$heading`n`n| Job / check | Conclusion | Observed error summary |`n| --- | --- | --- |"
        foreach ($failure in $Failures[$offset..([Math]::Min($offset + $jobsPerPage, $Failures.Count) - 1)]) {
            $name = Get-ScheduledTextExcerpt (ConvertTo-ScheduledTableCell $failure.name) 400
            $summary = Get-ScheduledTextExcerpt (ConvertTo-ScheduledTableCell $failure.summary) 400
            $table += "`n| [$($name.Replace("`n", ' '))]($($failure.url)) | $(ConvertTo-ScheduledTableCell $failure.conclusion) | $($summary.Replace("`n", ' ')) |"
        }
        $blocks.Add(@{ heading = $heading; text = "$table`n`n$metadata" })
    }
    foreach ($failure in $Failures) {
        $heading = "## Diagnostic excerpt: $($failure.url)"
        $name = Get-ScheduledTextExcerpt (ConvertTo-ScheduledTableCell $failure.name) 400
        # Allocate short metadata first, then share the remaining budget between verbose
        # sources. Clip each source only once so both of its ends survive final assembly.
        $parts = [string[]]::new($failure.diagnostics.Count)
        $remainingSources = $parts.Length
        $remaining = $diagnosticLimit - [Math]::Max(0, ($parts.Length - 1) * 2)
        if ($parts.Length -gt 0) {
            $indices = @(0..($parts.Length - 1) | Sort-Object { $failure.diagnostics[$_].Length })
            foreach ($index in $indices) {
                $share = [int][Math]::Floor($remaining / $remainingSources)
                $parts[$index] = Get-ScheduledTextExcerpt $failure.diagnostics[$index] $share
                $remaining -= $parts[$index].Length
                $remainingSources--
            }
        }
        $excerpt = $parts -join "`n`n"
        $text = (($excerpt -split '\r?\n' | ForEach-Object { "    $_" }) -join "`n")
        # Destinations come from the Actions artifact inventory, not diagnostic wording.
        $links = @(($failure['artifact_urls'] ?? @()) | Select-Object -Unique | ForEach-Object { "Result artifact: $_" })
        $blocks.Add(@{
            heading = $heading
            text = "$heading`n`n[$($name.Replace("`n", ' ')) - job and full logs]($($failure.url))`n`n$($links -join "`n")`n`n$text"
        })
    }
    $message = $prefix
    foreach ($block in $blocks) {
        $published = @($ExistingText | Where-Object {
            $normalized = $_.Replace("`r`n", "`n")
            $normalized.StartsWith("$prefix`n", [StringComparison]::Ordinal) -and
                $normalized.Contains("`n`n$($block.heading)`n`n", [StringComparison]::Ordinal)
        }).Count -gt 0
        # Keep the first published snapshot even if the run completes or diagnostics expire.
        # Its visible heading identifies a complete section, not a serialized publication log.
        if ($published) { continue }
        if ($prefix.Length + $block.text.Length + 2 -gt $messageLimit) {
            throw 'A bounded report section exceeds the GitHub message limit.'
        }
        if ($message.Length + $block.text.Length + 2 -gt $messageLimit) {
            $message
            $message = $prefix
        }
        $message += "`n`n$($block.text)"
    }
    if ($message -cne $prefix) { $message }
}

Export-ModuleMember -Function Get-ScheduledLogExcerpt, Format-ScheduledReport
