#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# In-process controller boundary: upload credentials must be absent during compilation and
# restored even when the compiler or artifact resolver fails. Native JSON/CLI is integration-tested.
BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'ReleaseBinaries.psm1') -Force
}

Describe 'Release controller compilation credentials' {
    It 'restores the caller credentials after compilation failure=<Fail>' -ForEach @(
        @{ Fail = $false }, @{ Fail = $true }
    ) {
        InModuleScope ReleaseBinaries -Parameters @{ Fail = $Fail } {
            param($Fail)
            Mock Push-Location {}
            Mock Pop-Location {}
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
                if ($Fail) {
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
