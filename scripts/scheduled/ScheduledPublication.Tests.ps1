#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }
# Exercises the reporter's write boundary against fake GitHub persistence and HTTP replies.
# Cooldowns are recorded, never slept; no test can publish to GitHub or depend on real time.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'
Import-Module (Join-Path $PSScriptRoot 'ScheduledGitHub.psm1') -Force

Describe 'Paced and reconciled GitHub writes' {
    InModuleScope ScheduledGitHub {
        BeforeEach {
            $script:savedExitCode = Get-Variable LASTEXITCODE -Scope Global -ValueOnly -ErrorAction SilentlyContinue
            $script:requests = 0
            $script:persisted = $null
            $script:events = [Collections.Generic.List[string]]::new()
            $script:failureOutput = "HTTP/2.0 403 Forbidden`n`n{`"message`":`"secondary rate limit`"}"
            $script:failures = 1
            Mock Start-Sleep {
                param($Seconds)
                $script:events.Add("wait:$Seconds")
            }
            Mock gh {
                $script:requests++
                $script:events.Add('write')
                if ($script:requests -le $script:failures) {
                    $global:LASTEXITCODE = 1
                    $script:failureOutput
                } else {
                    $global:LASTEXITCODE = 0
                    "HTTP/2.0 201 Created`nContent-Type: application/json`n`n{`"id`":1}"
                }
            }
            $script:findPersisted = {
                $script:events.Add('read')
                $script:persisted
            }
        }
        AfterEach { $global:LASTEXITCODE = $script:savedExitCode }

        It 'paces consecutive successful <Method> writes' -ForEach @(
            @{ Method = 'POST' }, @{ Method = 'PATCH' }
        ) {
            $script:failures = 0
            foreach ($index in 1..3) {
                (Invoke-ScheduledGitHubWrite repos/example/repo/issues -Method $Method `
                    -Body @{ body = "message $index" } -FindPersisted $script:findPersisted).id | Should -Be 1
            }
            $script:events | Should -Be @('wait:1', 'write', 'wait:1', 'write', 'wait:1', 'write')
        }
        It 'honors <Case> before reading or retrying a rejected write' -ForEach @(
            @{ Case = 'secondary throttling'; Headers = ''; Status = 403; Delay = 60 }
            @{ Case = 'Retry-After'; Headers = "Retry-After: 180`n"; Status = 403; Delay = 180 }
            @{ Case = 'HTTP 429'; Headers = ''; Status = 429; Delay = 60 }
            @{ Case = 'primary reset'; Headers = "X-RateLimit-Remaining: 0`nX-RateLimit-Reset: 2000`n"; Status = 403; Delay = 1001 }
        ) {
            Mock Get-Date { [datetimeoffset]::FromUnixTimeSeconds(1000).UtcDateTime }
            $script:failureOutput = "HTTP/2.0 $Status Forbidden`n${Headers}`n{`"message`":`"rate limit exceeded`"}"
            (Invoke-ScheduledGitHubWrite repos/example/repo/issues -Body @{ body = 'test' } `
                -FindPersisted $script:findPersisted).id | Should -Be 1
            $script:events | Should -Be @('wait:1', 'write', "wait:$Delay", 'read', 'wait:1', 'write')
        }
        It 'stops persistent throttling after finite exponential retries' {
            $script:failures = 3
            { Invoke-ScheduledGitHubWrite repos/example/repo/issues -Body @{ body = 'test' } `
                -FindPersisted $script:findPersisted } | Should -Throw
            $script:requests | Should -Be 3
            $script:events | Should -Be @(
                'wait:1', 'write', 'wait:60', 'read', 'wait:1', 'write', 'wait:120', 'read', 'wait:1', 'write'
            )
        }
        It 'does not replay <Case> when reconciliation finds no persisted effect' -ForEach @(
            @{ Case = 'a server failure'; Output = "HTTP/2.0 503 Service Unavailable`n`n{}" }
            @{ Case = 'a lost connection'; Output = 'connection reset by peer' }
            @{ Case = 'a permission refusal'; Output = "HTTP/2.0 403 Forbidden`n`n{`"message`":`"Resource not accessible`"}" }
        ) {
            $script:failureOutput = $Output
            { Invoke-ScheduledGitHubWrite repos/example/repo/issues -Body @{ body = 'test' } `
                -FindPersisted $script:findPersisted } | Should -Throw
            $script:requests | Should -Be 1
            $script:events | Should -Be @('wait:1', 'write', 'read')
        }
        It 'accepts observed persistence after <Case> without another write' -ForEach @(
            @{ Case = 'lost response'; Status = 1; Output = 'connection reset by peer' }
            @{ Case = 'malformed successful JSON'; Status = 0; Output = "HTTP/2.0 201 Created`n`ninvalid JSON" }
            @{ Case = 'missing successful headers'; Status = 0; Output = '{"id":1}' }
        ) {
            $script:replyStatus = $Status
            $script:replyOutput = $Output
            Mock gh {
                $script:requests++
                $script:events.Add('write')
                $script:persisted = @{ id = 1 }
                $global:LASTEXITCODE = $script:replyStatus
                $script:replyOutput
            }
            (Invoke-ScheduledGitHubWrite repos/example/repo/issues -Body @{ body = 'test' } `
                -FindPersisted $script:findPersisted).id | Should -Be 1
            $script:requests | Should -Be 1
            $script:events | Should -Be @('wait:1', 'write', 'read')
        }
        It 'does not retry when reconciliation itself fails after a cooldown' {
            { Invoke-ScheduledGitHubWrite repos/example/repo/issues -Body @{ body = 'test' } `
                -FindPersisted { throw [IO.IOException]::new() } } | Should -Throw
            $script:requests | Should -Be 1
            $script:events | Should -Be @('wait:1', 'write', 'wait:60')
        }
        It 'reuses an observed write after throttling instead of resending it' {
            $script:persisted = @{ id = 1 }
            (Invoke-ScheduledGitHubWrite repos/example/repo/issues -Body @{ body = 'test' } `
                -FindPersisted $script:findPersisted).id | Should -Be 1
            $script:requests | Should -Be 1
            $script:events | Should -Be @('wait:1', 'write', 'wait:60', 'read')
        }
    }
}

Describe 'Large report publication recovery' {
    InModuleScope ScheduledGitHub {
        BeforeAll {
            $script:directory = Join-Path $TestDrive 'report'
            $script:verboseSummary = "Replay: just mutants`n" + ("MISSED mutation at file.rs`n" * 40000) + 'Final failure'
        }
        BeforeEach {
            $script:issues = @()
            $script:comments = [Collections.Generic.List[hashtable]]::new()
            $script:commentAttempts = 0
            $script:issueAttempts = 0
            $script:failAt = 2
            $script:mode = 'throttle'
            $script:expired = $false
            Mock Start-Sleep {}
            Mock Invoke-ScheduledGitHubJson {
                param($Endpoint, $Method, $Body)
                if ($Method -ceq 'POST') {
                    if ($Endpoint -ceq 'repos/example/repo/issues') {
                        $script:issueAttempts++
                        $script:issues = @(@{
                            number = 50; body = $Body.body; comments = 0; state = 'open'
                            html_url = 'https://github.com/example/repo/issues/50'
                        })
                        return $script:issues[0]
                    }
                    if ($Endpoint -ceq 'repos/example/repo/issues/50/comments') {
                        $script:commentAttempts++
                        if ($script:commentAttempts -eq $script:failAt -and $script:mode -ne 'lost') {
                            $failure = [IO.IOException]::new()
                            if ($script:mode -eq 'throttle') {
                                $failure.Data['ScheduledThrottleResponse'] = "HTTP/2.0 403 Forbidden`n`nsecondary rate limit"
                            }
                            throw $failure
                        }
                        $comment = @{ body = $Body.body }
                        $script:comments.Add($comment)
                        if ($script:commentAttempts -eq $script:failAt -and $script:mode -eq 'lost') {
                            throw [IO.IOException]::new()
                        }
                        return $comment
                    }
                    throw "Unexpected write: $Endpoint"
                }
                switch -Regex ($Endpoint) {
                    '/actions/runs/10$' { return @{
                        id = 10; run_attempt = 1; head_sha = 'a' * 40; name = 'Deep validation'
                        status = $(if ($script:expired) { 'completed' } else { 'in_progress' })
                        run_started_at = '2026-09-11T00:00:00Z'
                    } }
                    '/jobs\?filter=all&per_page=100&page=1$' { return @{ jobs = @(1..30 | ForEach-Object { @{
                        id = $_; run_id = 10; run_attempt = 1; name = "check-$_"
                        html_url = "https://github.com/example/repo/actions/runs/10/job/$_"
                        status = 'completed'; conclusion = 'failure'; steps = @()
                    } }) } }
                    '/artifacts\?per_page=100&page=1$' { return @{ artifacts = @(1..30 | ForEach-Object { @{
                        id = $_; name = "scheduled-result-10-1-check-$_"; expired = $script:expired
                    } }) } }
                    '/issues\?state=all&labels=scheduled-run-failure&per_page=100&page=1$' { return ,$script:issues }
                    '/issues/50/comments\?per_page=100&page=1$' { return ,$script:comments.ToArray() }
                    '/labels\?per_page=100&page=1$' { return ,@(@{ name = 'scheduled-run-failure' }) }
                    default { throw "Unexpected read: $Endpoint" }
                }
            }
            Mock Save-ScheduledGitHubFile {
                param($Path)
                if ($script:expired) { throw [IO.IOException]::new() }
                Set-Content -LiteralPath $Path -Value '##[error]check failed'
            }
            Mock Read-ScheduledArtifactText {
                if ($script:expired) { throw [IO.IOException]::new() }
                $script:verboseSummary
            }
        }
        It 'finishes every job after a <Mode> continuation response without duplicates' -ForEach @(
            @{ Mode = 'throttle' }, @{ Mode = 'lost' }
        ) {
            $script:mode = $Mode
            Invoke-ScheduledReporting example/repo 10 1 $script:directory | Should -Match '/issues/50'
            $script:issueAttempts | Should -Be 1
            $script:comments.Count | Should -BeGreaterThan 1
            $script:comments.Count | Should -BeLessOrEqual 6
            $allText = (@($script:issues[0].body) + @($script:comments | ForEach-Object body)) -join "`n"
            foreach ($index in 1..30) {
                ([regex]::Matches($allText, "(?m)^## Diagnostic excerpt: https://github.com/example/repo/actions/runs/10/job/$index$")).Count |
                    Should -Be 1
                $allText | Should -Match "/artifacts/$index\b"
            }
            $savedCount = $script:commentAttempts
            $script:expired = $true
            Invoke-ScheduledReporting example/repo 10 1 $script:directory | Should -Match '/issues/50'
            $script:issueAttempts | Should -Be 1
            $script:commentAttempts | Should -Be $savedCount
        }
        It 'resumes partial progress after an unresolved ambiguous write without replacing the issue' {
            $script:mode = 'ambiguous'
            { Invoke-ScheduledReporting example/repo 10 1 $script:directory } | Should -Throw
            $script:issueAttempts | Should -Be 1
            $script:commentAttempts | Should -Be 2
            $script:comments.Count | Should -Be 1
            $savedBody = $script:issues[0].body
            $savedComment = $script:comments[0].body
            $script:expired = $true
            Invoke-ScheduledReporting example/repo 10 1 $script:directory | Should -Match '/issues/50'
            $script:issues[0].body | Should -BeExactly $savedBody
            $script:comments[0].body | Should -BeExactly $savedComment
            $script:issueAttempts | Should -Be 1
            $allText = (@($savedBody) + @($script:comments | ForEach-Object body)) -join "`n"
            foreach ($index in 1..30) {
                ([regex]::Matches($allText, "(?m)^## Diagnostic excerpt: https://github.com/example/repo/actions/runs/10/job/$index$")).Count |
                    Should -Be 1
            }
        }
    }
}
