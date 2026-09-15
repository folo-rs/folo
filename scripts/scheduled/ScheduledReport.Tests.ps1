#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }
# Protects human-readable reporting: preserve every failure, exclude successful-job noise,
# and bound verbose diagnostics without dropping jobs or publishing encoded records.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'ScheduledReport.psm1') -Force
}

Describe 'Readable failure rendering' {
    BeforeEach {
        $script:run = @{
            name = 'Deep validation'; run_started_at = '2026-09-11T01:02:03Z'
            status = 'in_progress'; head_sha = 'a' * 40
        }
        $script:attempt = 'https://github.com/example/repo/actions/runs/10/attempts/1'
        $script:failure = @{
            name = 'miri-linux'; url = "$attempt/jobs/20"
            conclusion = 'failure'; summary = 'error: test failed'
            diagnostics = @("test example::failed_test ... FAILED`nseed: 17`nReplay: just package=events miri")
        }
    }
    It 'renders identity, linked failed-job table and useful diagnostics' {
        $text = @(Format-ScheduledReport $run $attempt @($failure)) -join "`n"
        $text | Should -Match '^\[Copilot speaking\]'
        $text | Should -Match ([regex]::Escape($attempt))
        $text | Should -Match 'UTC start: 2026-09-11 01:02:03'
        $text | Should -Match 'a{40}'
        $text | Should -Match '\| Job / check \| Conclusion \| Observed error summary \|'
        $text | Should -Match 'Workflow status: in_progress'
        $text | Should -Match 'example::failed_test'
        $text | Should -Match 'seed: 17'
        $text | Should -Match 'Replay: just package=events miri'
        $text | Should -Not -Match '<!--|base64|schema_version'
    }
    It 'bounds verbose diagnostics while retaining their start, end and omission notice' {
        $failure.diagnostics = @((1..2000 | ForEach-Object {
            "MISSED mutant $_ in example::operation - replace return expression with a different value"
        }) -join "`n")
        $messages = @(Format-ScheduledReport $run $attempt @($failure))
        $messages.Count | Should -Be 1
        foreach ($message in $messages) {
            $message.Length | Should -BeLessOrEqual 60000
            $message | Should -Match '^\[Copilot speaking\]'
            $message | Should -Match ([regex]::Escape($attempt))
        }
        $text = $messages -join "`n"
        $text.Length | Should -BeLessThan 12000
        $text | Should -Match 'MISSED mutant 1 in'
        $text | Should -Match 'MISSED mutant 2000 in'
        $text | Should -Match 'Diagnostic text omitted'
        $text | Should -Not -Match '<!--|base64|schema_version'
    }
    It 'escapes table content and keeps artifact HTML visible rather than active' {
        $failure.name = 'miri | <script>'
        $failure.diagnostics = @('<!-- not an ownership marker -->')
        $text = @(Format-ScheduledReport $run $attempt @($failure)) -join "`n"
        $text | Should -Match 'miri &#124; &lt;script&gt;'
        $text | Should -Match '(?m)^    <!-- not an ownership marker -->'
    }
    It 'keeps every failed job when the unsuccessful-job table spans comments' {
        $failures = @(1..250 | ForEach-Object { @{
            name = "check-$_"; url = "$attempt/jobs/$_"
            conclusion = 'failure'; summary = 'An unsuccessful step was observed.'
            diagnostics = @("Observed error for check $_")
        } })
        $messages = @(Format-ScheduledReport $run $attempt $failures)
        $messages.Count | Should -BeGreaterThan 1
        @([regex]::Matches(($messages -join "`n"), '\| \[check-(\d+)\]') | ForEach-Object {
            [int]$_.Groups[1].Value
        }) | Should -Be (1..250)
    }
    It 'bounds large multi-job reports independently of source verbosity' {
        $failures = @(1..10 | ForEach-Object { @{
            name = "check-$_"; url = "$attempt/jobs/$_"
            conclusion = 'failure'; summary = "error in check $_"
            diagnostics = @("Replay: just check-$_`n" + ("MISSED mutant`n" * 100000) + "Final failure $_")
        } })
        $messages = @(Format-ScheduledReport $run $attempt $failures)
        $messages.Count | Should -BeLessOrEqual 3
        foreach ($message in $messages) { $message.Length | Should -BeLessOrEqual 60000 }
        $text = $messages -join "`n"
        foreach ($index in 1..10) {
            $text | Should -Match "Replay: just check-$index\b"
            $text | Should -Match "Final failure $index\b"
            $text | Should -Match ([regex]::Escape("[$("check-$index")]($attempt/jobs/$index)"))
        }
    }
    It 'keeps whole sections and artifact destinations even for densely indented diagnostics' {
        $failure.diagnostics = @(("start`n" + ("`n" * 100000) + 'end'),
            'Result artifact: https://github.com/example/repo/actions/runs/10/artifacts/30',
            ("x" * 100000))
        $messages = @(Format-ScheduledReport $run $attempt @($failure))
        $messages.Count | Should -Be 1
        $messages[0].Length | Should -BeLessOrEqual 60000
        $messages[0] | Should -Match '/artifacts/30'
        $messages[0] | Should -Match 'start'
        $messages[0] | Should -Match 'end'
    }
    It 'resumes missing sections without repeating completed snapshots after diagnostics change' {
        $failures = @(1..30 | ForEach-Object { @{
            name = "check-$_"; url = "$attempt/jobs/$_"; conclusion = 'failure'; summary = 'error'
            diagnostics = @("Replay $_`n" + ('failed result line ' * 1000) + "`nFinal $_")
        } })
        $messages = @(Format-ScheduledReport $run $attempt $failures)
        $messages.Count | Should -BeGreaterThan 2
        $run.status = 'completed'
        foreach ($item in $failures) { $item.diagnostics = @('Artifact expired; job logs unavailable.') }
        $remaining = @(Format-ScheduledReport $run $attempt $failures -ExistingText @($messages[0]))
        $combined = (@($messages[0]) + $remaining) -join "`n"
        foreach ($index in 1..30) {
            ([regex]::Matches($combined, "(?m)^## Diagnostic excerpt: $([regex]::Escape("$attempt/jobs/$index"))$")).Count |
                Should -Be 1
        }
        @(Format-ScheduledReport $run $attempt $failures -ExistingText (@($messages[0]) + $remaining)).Count |
            Should -Be 0
    }
    It 'does not split surrogate pairs at excerpt boundaries' {
        $failure.diagnostics = @([string]::Concat([char]0xD83D, [char]0xDE00) * 10000)
        $message = @(Format-ScheduledReport $run $attempt @($failure))[0]
        $encoding = [Text.UTF8Encoding]::new($false, $true)
        { $null = $encoding.GetBytes($message) } | Should -Not -Throw
    }
}

Describe 'Failure log excerpts' {
    It 'keeps distant error contexts without copying entire successful steps' {
        $lines = @('useful setup context', '##[error]toolchain download failed', 'HTTP 503')
        $lines += @(1..50 | ForEach-Object { "successful dependency $_" })
        $lines += @('test example::late_failure', 'thread panicked: undefined behavior', 'seed 37')
        $text = Get-ScheduledLogExcerpt ($lines -join "`n")
        $text | Should -Match 'HTTP 503'
        $text | Should -Match 'late_failure'
        $text | Should -Match 'seed 37'
        $text | Should -Not -Match 'successful dependency 25'
        $text | Should -Match 'Other log lines omitted'
    }
    It 'retains an unfamiliar failure tail and strips terminal control sequences' {
        $text = Get-ScheduledLogExcerpt ("`e[31munknown termination`e[0m")
        $text | Should -BeExactly 'unknown termination'
    }
    It 'does not clip a long error list to its first matches' {
        $text = Get-ScheduledLogExcerpt ((1..300 | ForEach-Object { "MISSED mutant $_" }) -join "`n")
        $text | Should -Match 'MISSED mutant 1'
        $text | Should -Match 'MISSED mutant 300'
    }
}
