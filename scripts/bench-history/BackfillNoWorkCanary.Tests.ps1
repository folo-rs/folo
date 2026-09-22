#Requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Exercises the hosted no-work verifier against synthetic GitHub job evidence,
# including failed preparation and accidentally executed benchmark jobs.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    $script:Verify = Join-Path $PSScriptRoot 'Assert-BackfillNoWorkCanary.ps1'
    function gh { throw 'The GitHub boundary must be mocked.' }
}

Describe 'No-work backfill caller verification' {
    BeforeEach {
        $script:Invocation = @{
            Repository = 'fixture/repository'
            RunId = 123
            RunAttempt = 2
            OutputPath = Join-Path $TestDrive 'jobs.json'
        }
        $script:Evidence = @{
            pages = @(
                @{ jobs = @(@{ name = 'no-eligible-backfill / prepare'; conclusion = 'success' }) }
                @{ jobs = @(
                        @{ name = 'no-eligible-backfill / backfill'; conclusion = 'skipped' }
                        @{ name = 'backfill / backfill (ubuntu-latest)'; conclusion = 'success' }
                    ) }
            )
        }
        $evidence = $script:Evidence
        Mock gh -MockWith ({
            ConvertTo-Json -InputObject $evidence.pages -Depth 6
            $global:LASTEXITCODE = 0
        }.GetNewClosure())
    }

    It 'queries the current attempt and accepts unrelated executed work' {
        & $script:Verify @script:Invocation
        Should -Invoke gh -Times 1 -Exactly -ParameterFilter {
            ($args -contains '--paginate') -and ($args -contains '--slurp') -and
            ($args -contains 'repos/fixture/repository/actions/runs/123/attempts/2/jobs?per_page=100')
        }
        (Get-Content -LiteralPath $script:Invocation.OutputPath -Raw | ConvertFrom-Json).Count | Should -Be 2
    }

    It 'accepts an unexpanded matrix without a skipped-job record' {
        $script:Evidence.pages = @($script:Evidence.pages[0])
        { & $script:Verify @script:Invocation } | Should -Not -Throw
    }

    It 'rejects <Case> rather than claiming a successful no-work result' -ForEach @(
        @{ Case = 'missing preparation' }
        @{ Case = 'failed preparation' }
        @{ Case = 'duplicate preparation' }
        @{ Case = 'executed backfill' }
        @{ Case = 'pending backfill' }
    ) {
        switch ($Case) {
            'missing preparation' { $script:Evidence.pages[0].jobs = @() }
            'failed preparation' { $script:Evidence.pages[0].jobs[0].conclusion = 'failure' }
            'duplicate preparation' { $script:Evidence.pages[0].jobs += $script:Evidence.pages[0].jobs[0] }
            'executed backfill' { $script:Evidence.pages[1].jobs[0].conclusion = 'success' }
            'pending backfill' { $script:Evidence.pages[1].jobs[0].conclusion = $null }
        }
        { & $script:Verify @script:Invocation } | Should -Throw
        Test-Path -LiteralPath $script:Invocation.OutputPath | Should -BeTrue
    }

    It 'propagates unavailable GitHub evidence' {
        Mock gh { throw 'Synthetic GitHub failure.' }
        { & $script:Verify @script:Invocation } | Should -Throw
    }
}
