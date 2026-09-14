#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Exercises the full scheduled summary assembly and upload with synthetic child output.
# Protects GitHub's payload boundary without running expensive mutation workloads.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'ScheduledExecution.psm1') -Force
    $script:sourceRoot = Join-Path $TestDrive 'source'
    $null = New-Item -ItemType Directory -Path $sourceRoot
    $script:sourceSha = 'a' * 40
    # GitHub's documented per-step upload limit, independent of implementation constants.
    $script:uploadLimit = 1MB
    $script:strictUtf8 = [Text.UTF8Encoding]::new($false, $true)
}

Describe 'Complete scheduled step-summary payload' {
    BeforeEach {
        $script:output = Join-Path $TestDrive ([guid]::NewGuid().ToString())
        $script:previousStepSummary = $env:GITHUB_STEP_SUMMARY
        $env:GITHUB_STEP_SUMMARY = Join-Path $TestDrive ("upload-$([guid]::NewGuid()).md")
        # The wrapper owns its step; publishing must not append to stale payload bytes.
        [IO.File]::WriteAllText($env:GITHUB_STEP_SUMMARY, 'Earlier payload')
        $script:check = @{
            id = 'mutants-summary'; platform = 'test'; recipe = 'mutants'; packages = @(); shard = '1/1'
        }
    }

    AfterEach {
        $env:GITHUB_STEP_SUMMARY = $previousStepSummary
    }

    It 'bounds saturated diagnostics with exit <Code>, UTF-8 scalar <Scalar> and <NewlineName> input' -ForEach @(
        @{ Code = 0; Scalar = 0x78; Newline = "`n"; NewlineName = 'LF' },
        @{ Code = 3; Scalar = 0xE9; Newline = "`r`n"; NewlineName = 'CRLF' },
        @{ Code = 3; Scalar = 0x20AC; Newline = "`n"; NewlineName = 'LF' },
        @{ Code = 0; Scalar = 0x1F680; Newline = "`r`n"; NewlineName = 'CRLF' }
    ) {
        $script:recipeExit = $Code
        $script:logText = "Diagnostic canary$Newline" +
            (([char]::ConvertFromUtf32($Scalar) + $Newline) * 32768)
        Mock Invoke-CapturedProcess -ModuleName ScheduledExecution {
            param($StandardOutputPath, $StandardErrorPath)
            [IO.File]::WriteAllText($StandardOutputPath, 'Recipe stdout canary')
            [IO.File]::WriteAllText($StandardErrorPath, '')
            $null = New-Item -ItemType Directory -Path (Join-Path $output 'mutants.out\log')
            # Enough distinct native logs to saturate the real aggregate budget after excerpting.
            $outcomes = @(1..20 | ForEach-Object {
                [IO.File]::WriteAllText((Join-Path $output "mutants.out\log\$_.log"), $logText)
                @{
                    scenario = @{ Mutant = @{ name = "replace expression $_"; package = 'example'; file = 'lib.rs'; replacement = '0' } }
                    summary = 'MissedMutant'; phase_results = @(); log_path = "log/$_.log"
                }
            })
            @{ outcomes = $outcomes; total_mutants = 20; missed = 20; timeout = 0 } |
                ConvertTo-Json -Depth 20 | Set-Content -LiteralPath (Join-Path $output 'mutants.out\outcomes.json')
            return $recipeExit
        }

        Invoke-ScheduledCheck -Check $check -SourceRoot $sourceRoot -OutputDirectory $output -SourceSha $sourceSha |
            Should -Be $Code

        $summaryPath = Join-Path $output 'summary.md'
        $bytes = [IO.File]::ReadAllBytes($env:GITHUB_STEP_SUMMARY)
        $bytes.Length | Should -BeLessOrEqual $uploadLimit
        $bytes.Length | Should -BeGreaterThan ($uploadLimit - 1KB)
        # Comparing bytes catches BOM insertion and line-ending conversion during upload.
        [Convert]::ToBase64String($bytes) |
            Should -BeExactly ([Convert]::ToBase64String([IO.File]::ReadAllBytes($summaryPath)))
        $text = $strictUtf8.GetString($bytes)
        $text | Should -Match $sourceSha
        $text | Should -Match 'Diagnostic canary'
        $text | Should -Match 'Full log in result artifact: mutants.out[/\\]log[/\\]1.log'
        @([regex]::Matches($text, 'Diagnostic summary truncated')).Count | Should -Be 1
        $conclusion = if ($Code -eq 0) { 'PASSED' } else { 'FAILED' }
        $text | Should -Match "## Final result: $conclusion\r?\n\r?\nExit code: $Code\r?\n$"
        [IO.File]::ReadAllText((Join-Path $output 'mutants.out\log\20.log')) | Should -BeExactly $logText
    }

    It 'bounds an oversized execution failure while retaining its full artifact and failed result' {
        $script:failureText = 'Capture failure canary ' + ('x' * $uploadLimit)
        Mock Invoke-CapturedProcess -ModuleName ScheduledExecution { throw $failureText }

        Invoke-ScheduledCheck -Check $check -SourceRoot $sourceRoot -OutputDirectory $output -SourceSha $sourceSha |
            Should -Be 1

        $bytes = [IO.File]::ReadAllBytes($env:GITHUB_STEP_SUMMARY)
        $bytes.Length | Should -BeLessOrEqual $uploadLimit
        $text = $strictUtf8.GetString($bytes)
        $text | Should -Match 'Capture failure canary'
        $text | Should -Match 'Diagnostic summary truncated'
        $text | Should -Match 'result artifact'
        $text | Should -Match '## Final result: FAILED\r?\n\r?\nExit code: 1\r?\n$'
        $errorText = [IO.File]::ReadAllText((Join-Path $output 'execution-error.txt'))
        $errorText | Should -Match 'Capture failure canary'
        # PowerShell's error view can insert a line break after the canary.
        $errorText.Contains('x' * $uploadLimit) | Should -BeTrue
    }

    It 'preserves a small summary without truncation or upload transcoding' {
        Mock Invoke-CapturedProcess -ModuleName ScheduledExecution {
            param($StandardOutputPath, $StandardErrorPath)
            [IO.File]::WriteAllText($StandardOutputPath, '')
            [IO.File]::WriteAllText($StandardErrorPath, '')
            return 0
        }

        Invoke-ScheduledCheck -Check $check -SourceRoot $sourceRoot -OutputDirectory $output -SourceSha $sourceSha |
            Should -Be 0

        $bytes = [IO.File]::ReadAllBytes($env:GITHUB_STEP_SUMMARY)
        [Convert]::ToBase64String($bytes) |
            Should -BeExactly ([Convert]::ToBase64String([IO.File]::ReadAllBytes((Join-Path $output 'summary.md'))))
        $text = $strictUtf8.GetString($bytes)
        $text | Should -Not -Match 'truncated'
        $text | Should -Match 'details are unavailable'
        $text | Should -Match '## Final result: PASSED\r?\n\r?\nExit code: 0\r?\n$'
    }
}

Describe 'Final summary byte boundaries' {
    It 'preserves the complete body with <SpareBytes> bytes left after the footer' -ForEach @(
        @{ SpareBytes = 0 }, @{ SpareBytes = 1 }
    ) {
        InModuleScope ScheduledExecutionDiagnostics -Parameters @{ Root = $TestDrive; SpareBytes = $SpareBytes } {
            param($Root, $SpareBytes)
            $path = Join-Path $Root 'boundary.md'
            $footer = "`n## Final result: FAILED`n`nExit code: 3`n"
            $body = 'x' * ($script:SummaryByteLimit - [Text.Encoding]::UTF8.GetByteCount($footer) - $SpareBytes)
            [IO.File]::WriteAllText($path, $body)
            Complete-ScheduledSummary -SummaryPath $path -Footer $footer
            ([IO.File]::ReadAllText($path) -ceq ($body + $footer)) | Should -BeTrue
        }
    }

    It 'backs up from byte <Offset> within a multibyte character when reserving the footer' -ForEach @(
        @{ Offset = 1 }, @{ Offset = 2 }, @{ Offset = 3 }
    ) {
        InModuleScope ScheduledExecutionDiagnostics -Parameters @{ Root = $TestDrive; Offset = $Offset } {
            param($Root, $Offset)
            $path = Join-Path $Root 'utf8-boundary.md'
            $footer = "`n## Final result: FAILED`n`nExit code: 3`n"
            $budget = $script:SummaryByteLimit - [Text.Encoding]::UTF8.GetByteCount($footer + $script:SummaryTruncationText)
            $prefix = 'x' * ($budget - $Offset)
            $body = $prefix + [char]::ConvertFromUtf32(0x1F680) + ('y' * $script:SummaryByteLimit)
            [IO.File]::WriteAllText($path, $body)
            Complete-ScheduledSummary -SummaryPath $path -Footer $footer
            $text = [Text.UTF8Encoding]::new($false, $true).GetString([IO.File]::ReadAllBytes($path))
            ($text -ceq ($prefix + $script:SummaryTruncationText + $footer)) | Should -BeTrue
        }
    }
}
