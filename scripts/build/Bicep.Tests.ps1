#Requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Protects the offline Bicep gate's diagnostic verdicts without invoking a compiler or Azure.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeDiscovery {
    Import-Module (Join-Path $PSScriptRoot 'Bicep.psm1') -Force
}

Describe 'Bicep diagnostic policy' {
    InModuleScope Bicep {
        It 'accepts an empty successful diagnostic set' {
            @(Get-BicepDiagnosticFailure -Diagnostics '{"version":"2.1.0","runs":[{"results":[]}]}') |
                Should -BeNullOrEmpty
        }

        It 'rejects compiler and linter diagnostics at <Level> level' -ForEach @(
            @{ Level = 'error' }, @{ Level = 'warning' }, @{ Level = $null }
        ) {
            $result = @{ ruleId = 'canary-rule'; message = @{ text = 'diagnostic canary' } }
            if ($null -ne $Level) { $result.level = $Level }
            $document = @{ version = '2.1.0'; runs = @(@{ results = @($result) }) } |
                ConvertTo-Json -Depth 8
            @(Get-BicepDiagnosticFailure -Diagnostics $document) |
                Should -Be @('canary-rule: diagnostic canary')
        }

        It 'does not promote informational diagnostics to failures' {
            $document = @{
                version = '2.1.0'
                runs = @(@{ results = @(@{ level = 'note'; message = @{ text = 'information' } }) })
            } | ConvertTo-Json -Depth 8
            @(Get-BicepDiagnosticFailure -Diagnostics $document) | Should -BeNullOrEmpty
        }

        It 'fails explicitly on malformed compiler diagnostics: <_>' -ForEach @(
            '', '{}', '[]', '{"version":"other","runs":[]}',
            '{"version":"2.1.0","runs":[{}]}',
            '{"version":"2.1.0","runs":[{"results":[{"level":"unexpected"}]}]}'
        ) {
            { Get-BicepDiagnosticFailure -Diagnostics $_ } | Should -Throw
        }
    }
}

Describe 'Bicep installation reuse' {
    InModuleScope Bicep {
        BeforeAll {
            function script:az { throw 'Installation must be mocked.' }
        }
        It 'uses the already installed pinned compiler without installation' {
            Mock Get-FoloBicepPath { 'compiler-canary' }
            Mock Test-Path { $true }
            Mock Invoke-BicepProcess {
                [pscustomobject]@{ ExitCode = 0; Stdout = 'Bicep CLI version 1.2.3 (canary)'; Stderr = '' }
            }
            Mock az { throw 'unexpected installation' }

            Install-FoloBicep -Version '1.2.3'
            Should -Invoke az -Times 0 -Exactly
            Should -Invoke Invoke-BicepProcess -Times 1 -Exactly -ParameterFilter {
                $Arguments.Count -eq 1 -and $Arguments[0] -eq '--version'
            }
        }
    }
}
