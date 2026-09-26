#requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Protects the local Just adapter's argument and failure propagation. Application tests own
# version, publication metadata and compatibility behavior; this boundary performs no policy.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'ReleasePlan.psm1') -Force
}

Describe 'Local release validation' {
    It 'forwards an explicit baseline without resolving or repairing inputs' {
        $result = Invoke-ReleaseValidation -Base 'baseline with spaces' -Cargo {
            param([string[]] $Argument)
            $Argument | Should -Be @('run', '--quiet', '--locked', '-p', 'cargo-release-plan', '--',
                'check', '--config', '.cargo/release_plan.toml', '--format', 'github', '--verbose',
                '--base', 'baseline with spaces')
            $global:LASTEXITCODE = 0
            'checked source'
        }
        $result | Should -Be 'checked source'
    }

    It 'leaves an unspecified baseline to the application' {
        Invoke-ReleaseValidation -Base '' -Cargo {
            param([string[]] $Argument)
            $Argument | Should -Not -Contain '--base'
            $global:LASTEXITCODE = 0
        }
    }

    It 'propagates failed execution instead of presenting successful output' {
        {
            Invoke-ReleaseValidation -Base '' -Cargo {
                $global:LASTEXITCODE = 1
                'partial output'
            }
        } | Should -Throw
    }
}
