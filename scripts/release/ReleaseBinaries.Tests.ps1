#requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# In-process controller boundary: upload credentials must be absent during compilation and
# restored independently of compiler and location-cleanup failures. Native JSON/CLI is integration-tested.
BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'ReleaseBinaries.psm1') -Force
}

Describe 'Release controller compilation credentials' {
    It 'restores credentials before cleanup; compilation failure=<Fail>; location failure=<CleanupFails>' -ForEach @(
        @{ Fail = $false; CleanupFails = $false },
        @{ Fail = $true; CleanupFails = $false },
        @{ Fail = $false; CleanupFails = $true },
        @{ Fail = $true; CleanupFails = $true }
    ) {
        InModuleScope ReleaseBinaries -Parameters @{ Fail = $Fail; CleanupFails = $CleanupFails } {
            param($Fail, $CleanupFails)
            Mock Push-Location {}
            Mock Pop-Location {
                foreach ($name in @('GH_TOKEN', 'GITHUB_TOKEN', 'GIT_TOKEN', 'INPUT_TOKEN', 'DEFAULT_GITHUB_TOKEN')) {
                    [Environment]::GetEnvironmentVariable($name) | Should -Be 'credential-filter-canary'
                }
                if ($CleanupFails) { throw 'location failure canary' }
            }
            Mock Resolve-CargoExecutable { 'controller.exe' }
            Mock cargo {
                foreach ($name in @('GH_TOKEN', 'GITHUB_TOKEN', 'GIT_TOKEN', 'INPUT_TOKEN', 'DEFAULT_GITHUB_TOKEN')) {
                    [Environment]::GetEnvironmentVariable($name) | Should -BeNullOrEmpty
                }
                if ($Fail) { throw 'compiler failure canary' }
                'fixture artifact'
            }
            $saved = @{}
            try {
                foreach ($name in @('GH_TOKEN', 'GITHUB_TOKEN', 'GIT_TOKEN', 'INPUT_TOKEN', 'DEFAULT_GITHUB_TOKEN')) {
                    $saved[$name] = [Environment]::GetEnvironmentVariable($name)
                    [Environment]::SetEnvironmentVariable($name, 'credential-filter-canary')
                }
                if ($CleanupFails) {
                    { Get-ReleaseBinariesExecutable } | Should -Throw '*location failure canary*'
                } elseif ($Fail) {
                    { Get-ReleaseBinariesExecutable } | Should -Throw '*compiler failure canary*'
                } else {
                    Get-ReleaseBinariesExecutable | Should -Be 'controller.exe'
                }
                foreach ($name in $saved.Keys) {
                    [Environment]::GetEnvironmentVariable($name) | Should -Be 'credential-filter-canary'
                }
                Should -Invoke cargo -Times 1 -Exactly
                Should -Invoke Pop-Location -Times 1 -Exactly
            } finally {
                foreach ($name in $saved.Keys) {
                    [Environment]::SetEnvironmentVariable($name, $saved[$name])
                }
            }
        }
    }
}
