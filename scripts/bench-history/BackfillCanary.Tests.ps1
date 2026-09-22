#Requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Executes the real caller-verification step with a mocked Cargo boundary. The cases
# validate stored endpoint identities without Azure access or benchmark execution.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    Import-Module powershell-yaml -RequiredVersion 0.4.12 -ErrorAction Stop
    $path = Join-Path -Path $PSScriptRoot -ChildPath '..' -AdditionalChildPath '..', '.github', 'workflows', 'benchmark-action-canary.yml'
    $workflow = Get-Content -LiteralPath $path -Raw | ConvertFrom-Yaml
    $step = @($workflow.jobs['verify-backfill'].steps |
            Where-Object { $_['name'] -ceq 'Verify stored historical measurements' })
    if ($step.Count -ne 1) { throw 'Expected one backfill verification step.' }
    $script:Verify = [scriptblock]::Create($step[0].run)
    function cargo { throw 'The Cargo boundary must be mocked.' }
}

Describe 'Backfill caller report verification' {
    BeforeEach {
        $script:EnvironmentBefore = @{}
        foreach ($name in @('GITHUB_WORKSPACE', 'RUNNER_TEMP', 'CANARY_FROM', 'CANARY_TO')) {
            $script:EnvironmentBefore[$name] = [Environment]::GetEnvironmentVariable($name)
        }
        $env:GITHUB_WORKSPACE = Join-Path $TestDrive 'invocation workspace'
        $env:RUNNER_TEMP = $TestDrive
        $env:CANARY_FROM = 'b' * 40
        $env:CANARY_TO = 'a' * 40
        $script:Report = @{
            project = 'reusable-backfill-canary'
            sets = @(foreach ($target in @('x86_64-unknown-linux-gnu', 'x86_64-pc-windows-msvc', 'aarch64-apple-darwin')) {
                @{
                    engine = 'criterion'
                    target_triple = $target
                    series = 1
                    runs = 2
                    commits = @(
                        @{ commit = $env:CANARY_FROM; clean = 1; dirty = 0; runs = 1 }
                        @{ commit = $env:CANARY_TO; clean = 1; dirty = 0; runs = 1 }
                    )
                }
            })
        }
        Mock cargo {
            $jsonIndex = [array]::IndexOf($args, '--json')
            if ($jsonIndex -lt 0) { throw 'The verifier must request a structured report.' }
            $script:Report | ConvertTo-Json -Depth 8 |
                Set-Content -LiteralPath $args[$jsonIndex + 1] -Encoding utf8
            $global:LASTEXITCODE = 0
        }
    }

    AfterEach {
        foreach ($name in $script:EnvironmentBefore.Keys) {
            [Environment]::SetEnvironmentVariable($name, $script:EnvironmentBefore[$name])
        }
    }

    It 'queries every stored platform and machine key rather than the verifier host' {
        & $script:Verify
        Should -Invoke cargo -Times 1 -Exactly -ParameterFilter {
            ($args -contains 'list') -and ($args -contains 'runs') -and
            ($args[[array]::IndexOf($args, '--context') + 1] -ceq ('a' * 40)) -and
            ($args[[array]::IndexOf($args, '--base') + 1] -ceq ('a' * 40)) -and
            ($args[[array]::IndexOf($args, '--machine-key') + 1] -ceq 'all') -and
            ($args[[array]::IndexOf($args, '--target-triple') + 1] -ceq 'all') -and
            ($args[[array]::IndexOf($args, '--engine') + 1] -ceq 'criterion') -and
            ($args -contains '--no-dirty')
        }
    }

    It 'permits additional historical machine keys without replacing another target' {
        $script:Report.sets += @{
            engine = 'criterion'
            target_triple = 'x86_64-unknown-linux-gnu'
            series = 1
            runs = 3
            commits = @(@{ commit = 'c' * 40; clean = 3; dirty = 0; runs = 3 })
        }
        { & $script:Verify } | Should -Not -Throw
    }

    It 'rejects <Case> instead of treating it as successful historical coverage' -ForEach @(
        @{ Case = 'wrong project' }
        @{ Case = 'stale range with the same run counts' }
        @{ Case = 'missing start' }
        @{ Case = 'missing end' }
        @{ Case = 'dirty endpoint' }
        @{ Case = 'empty endpoint' }
        @{ Case = 'wrong engine' }
        @{ Case = 'missing target' }
        @{ Case = 'empty series' }
        @{ Case = 'empty store' }
    ) {
        switch ($Case) {
            'wrong project' { $script:Report.project = 'another-project' }
            'stale range with the same run counts' {
                foreach ($set in $script:Report.sets) {
                    $set.commits[0].commit = 'c' * 40
                    $set.commits[1].commit = 'd' * 40
                }
            }
            'missing start' { $script:Report.sets[2].commits = @($script:Report.sets[2].commits[1]) }
            'missing end' { $script:Report.sets[2].commits = @($script:Report.sets[2].commits[0]) }
            'dirty endpoint' {
                $script:Report.sets[2].commits[1].clean = 0
                $script:Report.sets[2].commits[1].dirty = 1
            }
            'empty endpoint' { $script:Report.sets[2].commits[1].runs = 0 }
            'wrong engine' { $script:Report.sets[2].engine = 'gungraun' }
            'missing target' { $script:Report.sets = @($script:Report.sets[0..1]) }
            'empty series' { $script:Report.sets[2].series = 0 }
            'empty store' { $script:Report.sets = @() }
        }
        { & $script:Verify } | Should -Throw
    }

    It 'requires the endpoints in the same comparable partition' {
        $end = $script:Report.sets[2].commits[1]
        $script:Report.sets[2].commits = @($script:Report.sets[2].commits[0])
        $script:Report.sets += @{
            engine = 'criterion'
            target_triple = 'aarch64-apple-darwin'
            series = 1
            runs = 1
            commits = @($end)
        }
        { & $script:Verify } | Should -Throw
    }

    It 'preserves a failing listing command as an execution failure' {
        Mock cargo { throw 'Synthetic listing failure.' }
        { & $script:Verify } | Should -Throw
    }
}
