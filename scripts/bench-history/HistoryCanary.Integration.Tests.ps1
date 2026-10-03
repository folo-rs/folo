#Requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Exercises the caller's output and real report-file boundary with deterministic bundles,
# including multiple machine partitions, missing targets and inconsistent collection evidence.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'HistoryCanary.psm1') -Force
}

Describe 'History caller output verification' {
    BeforeEach {
        $script:Output = @{
            Outcome = 'clean'
            State = 'clean'
            Notable = 'false'
            Regressions = '0'
            PartialPlatformCoverage = 'false'
            ArtifactId = '123'
            ArtifactUrl = 'https://github.com/fixture/repository/actions/runs/1/artifacts/123'
        }
    }

    It 'accepts <Outcome> analysis with complete platform collection' -ForEach @(
        @{ Outcome = 'clean'; State = 'clean' }
        @{ Outcome = 'insufficient_baseline'; State = 'inconclusive' }
        @{ Outcome = 'partial'; State = 'inconclusive' }
    ) {
        $script:Output.Outcome = $Outcome
        $script:Output.State = $State
        { Assert-HistoryCanaryOutput @script:Output } | Should -Not -Throw
    }

    It 'rejects invalid <Field> output <Value>' -ForEach @(
        @{ Field = 'Outcome'; Value = 'regressions' }
        @{ Field = 'Outcome'; Value = '' }
        @{ Field = 'State'; Value = 'failed' }
        @{ Field = 'Notable'; Value = 'true' }
        @{ Field = 'Regressions'; Value = '1' }
        @{ Field = 'PartialPlatformCoverage'; Value = 'true' }
        @{ Field = 'PartialPlatformCoverage'; Value = '' }
        @{ Field = 'ArtifactId'; Value = '0' }
        @{ Field = 'ArtifactId'; Value = 'invalid' }
        @{ Field = 'ArtifactUrl'; Value = ' ' }
    ) {
        $script:Output[$Field] = $Value
        { Assert-HistoryCanaryOutput @script:Output } | Should -Throw
    }
}

Describe 'History caller report verification' {
    BeforeEach {
        $script:ReportDirectory = Join-Path $TestDrive 'report'
        $null = New-Item -ItemType Directory -Path $script:ReportDirectory -Force
        foreach ($reportName in @('report.md', 'summary.md')) {
            'Synthetic report content' | Set-Content -LiteralPath (Join-Path $script:ReportDirectory $reportName)
        }
        $script:Report = @{
            mode = 'history'
            outcome = 'clean'
            notable = $false
            regressions = 0
            series = 3
            census = @{ in_scope = 3 }
            sets = @(
                @{ engine = 'criterion'; target_triple = 'x86_64-unknown-linux-gnu'; machine_key = 'shared'; runs = 20; series = 1; regressions = 0 }
                @{ engine = 'criterion'; target_triple = 'x86_64-pc-windows-msvc'; machine_key = 'windows'; runs = 20; series = 1; regressions = 0 }
                @{ engine = 'criterion'; target_triple = 'aarch64-apple-darwin'; machine_key = 'macos'; runs = 20; series = 1; regressions = 0 }
            )
        }
        $script:Invocation = @{ ReportDirectory = $script:ReportDirectory; ExpectedOutcome = 'clean' }
    }

    BeforeAll {
        function Write-Report {
            $script:Report | ConvertTo-Json -Depth 8 |
                Set-Content -LiteralPath (Join-Path $script:ReportDirectory 'report.json')
        }
        function Add-WindowsPartition {
            $script:Report.sets += @{
                engine = 'criterion'; target_triple = 'x86_64-pc-windows-msvc'
                machine_key = 'shared'; runs = 20; series = 1; regressions = 0
            }
            $script:Report.series++
            $script:Report.census.in_scope++
        }
    }

    It 'accepts one partition per expected target' {
        Write-Report
        { Assert-HistoryCanaryReport @script:Invocation } | Should -Not -Throw
    }

    It 'accepts extra machine partitions with <Outcome> analysis' -ForEach @(
        @{ Outcome = 'clean' }
        @{ Outcome = 'insufficient_baseline' }
        @{ Outcome = 'partial' }
    ) {
        Add-WindowsPartition
        $script:Report.outcome = $Outcome
        $script:Invocation.ExpectedOutcome = $Outcome
        if ($Outcome -ceq 'partial') {
            # Mirror the retained report: excluded ghosts and unjudged baseline points are
            # statistical coverage, not missing collection targets.
            $script:Report.sets[3].runs = 7
            $script:Report.census = @{
                in_scope = 4; total = 5; judged = 3; unjudged = 2; coverage = 'partial'
                reasons = @(@{ reason = 'ghost'; count = 1 }, @{ reason = 'too_few_points'; count = 1 })
            }
        }
        Write-Report
        { Assert-HistoryCanaryReport @script:Invocation } | Should -Not -Throw
    }

    It 'rejects a missing target even when extra partitions fill the expected target count' {
        Add-WindowsPartition
        $script:Report.sets = @($script:Report.sets | Where-Object target_triple -CNE 'aarch64-apple-darwin')
        $script:Report.series = 3
        $script:Report.census.in_scope = 3
        Write-Report
        { Assert-HistoryCanaryReport @script:Invocation } | Should -Throw
    }

    It 'rejects <Case> in the downloaded report' -ForEach @(
        @{ Case = 'wrong mode' }
        @{ Case = 'mismatched outcome' }
        @{ Case = 'notable result' }
        @{ Case = 'regression' }
        @{ Case = 'empty sets' }
        @{ Case = 'unexpected target' }
        @{ Case = 'wrong engine' }
        @{ Case = 'empty partition' }
        @{ Case = 'extra series within a partition' }
        @{ Case = 'missing runs' }
        @{ Case = 'partition regression' }
        @{ Case = 'missing machine key' }
        @{ Case = 'empty machine key' }
        @{ Case = 'blank machine key' }
        @{ Case = 'duplicate partition' }
        @{ Case = 'inconsistent census' }
        @{ Case = 'inconsistent series total' }
    ) {
        switch ($Case) {
            'wrong mode' { $script:Report.mode = 'branch' }
            'mismatched outcome' { $script:Report.outcome = 'partial' }
            'notable result' { $script:Report.notable = $true }
            'regression' { $script:Report.regressions = 1 }
            'empty sets' { $script:Report.sets = @() }
            'unexpected target' { $script:Report.sets[0].target_triple = 'unexpected' }
            'wrong engine' { $script:Report.sets[0].engine = 'gungraun' }
            'empty partition' { $script:Report.sets[0].series = 0 }
            'extra series within a partition' { $script:Report.sets[0].series = 2 }
            'missing runs' { $script:Report.sets[0].runs = 0 }
            'partition regression' { $script:Report.sets[0].regressions = 1 }
            'missing machine key' { $script:Report.sets[0].Remove('machine_key') }
            'empty machine key' { $script:Report.sets[0].machine_key = '' }
            'blank machine key' { $script:Report.sets[0].machine_key = ' ' }
            'duplicate partition' {
                $script:Report.sets += $script:Report.sets[0]
                $script:Report.series++
                $script:Report.census.in_scope++
            }
            'inconsistent census' { $script:Report.census.in_scope = 4 }
            'inconsistent series total' { $script:Report.series = 4 }
        }
        Write-Report
        { Assert-HistoryCanaryReport @script:Invocation } | Should -Throw
    }

    It 'rejects <Case> report file <Name>' -ForEach @(
        foreach ($name in @('report.md', 'report.json', 'summary.md')) {
            foreach ($case in @('missing', 'empty', 'whitespace')) { @{ Name = $name; Case = $case } }
        }
    ) {
        Write-Report
        $path = Join-Path $script:ReportDirectory $Name
        switch ($Case) {
            'missing' { Remove-Item -LiteralPath $path }
            'empty' { '' | Set-Content -LiteralPath $path }
            'whitespace' { ' ' | Set-Content -LiteralPath $path }
        }
        { Assert-HistoryCanaryReport @script:Invocation } | Should -Throw
    }

    It 'rejects malformed JSON' {
        '{' | Set-Content -LiteralPath (Join-Path $script:ReportDirectory 'report.json')
        { Assert-HistoryCanaryReport @script:Invocation } | Should -Throw
    }
}
