#requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Exercises controller credential restoration and private-protocol finalization with in-process
# compiler/controller mocks. Real CLI/JSON coverage belongs to the integration suite.
BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'ReleaseBinaries.psm1') -Force
    InModuleScope ReleaseBinaries {
        $script:CredentialCanaries = @(
            'GH_TOKEN', 'GITHUB_TOKEN', 'GIT_TOKEN', 'INPUT_TOKEN', 'DEFAULT_GITHUB_TOKEN',
            'CARGO_REGISTRY_TOKEN', 'CARGO_REGISTRIES_CRATES_IO_TOKEN',
            'cargo_registries_fixture_token', 'ACTIONS_ID_TOKEN_REQUEST_URL',
            'ACTIONS_ID_TOKEN_REQUEST_TOKEN'
        )
        function script:Invoke-ControllerFixture {}
    }
}

Describe 'Release controller compilation credentials' {
    It 'restores <Package> credentials and preserves failures: compiler=<Fail>, cleanup=<CleanupFails>' -ForEach @(
        @{ Package = 'release-binaries'; Fail = $false; CleanupFails = $false }
        @{ Package = 'release-binaries'; Fail = $true; CleanupFails = $false }
        @{ Package = 'release-binaries'; Fail = $false; CleanupFails = $true }
        @{ Package = 'release-binaries'; Fail = $true; CleanupFails = $true }
        @{ Package = 'release-target-check'; Fail = $false; CleanupFails = $false }
        @{ Package = 'release-target-check'; Fail = $true; CleanupFails = $false }
        @{ Package = 'release-target-check'; Fail = $false; CleanupFails = $true }
        @{ Package = 'release-target-check'; Fail = $true; CleanupFails = $true }
    ) {
        InModuleScope ReleaseBinaries -Parameters @{
            Package = $Package; Fail = $Fail; CleanupFails = $CleanupFails
        } {
            param($Package, $Fail, $CleanupFails)
            Mock Push-Location {}
            Mock Pop-Location {
                foreach ($name in $script:CredentialCanaries) {
                    [Environment]::GetEnvironmentVariable($name, 'Process') | Should -Be 'credential-filter-canary'
                }
                if ($CleanupFails) { throw 'location failure canary' }
            }
            Mock Resolve-CargoExecutable { 'controller.exe' }
            Mock cargo {
                foreach ($name in $script:CredentialCanaries) {
                    $null -eq [Environment]::GetEnvironmentVariable($name, 'Process') | Should -BeTrue
                }
                [Environment]::GetEnvironmentVariable('CARGO_REGISTRIES_FIXTURE_INDEX', 'Process') |
                    Should -Be 'registry-index-canary'
                $args | Should -Contain '--locked'
                $args | Should -Contain $Package
                if ($Fail) { throw 'compiler failure canary' }
                'fixture artifact'
            }
            $saved = [Collections.Generic.Dictionary[string, object]]::new([StringComparer]::Ordinal)
            try {
                foreach ($name in @($script:CredentialCanaries) + @('CARGO_REGISTRIES_FIXTURE_INDEX')) {
                    $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
                }
                foreach ($name in $script:CredentialCanaries) {
                    [Environment]::SetEnvironmentVariable($name, 'credential-filter-canary', 'Process')
                }
                [Environment]::SetEnvironmentVariable('CARGO_REGISTRIES_FIXTURE_INDEX', 'registry-index-canary', 'Process')
                $failure = $null
                try { $result = Get-ReleaseControllerExecutable -Package $Package } catch { $failure = $_.Exception }
                if ($Fail -and $CleanupFails) {
                    $failure | Should -BeOfType ([AggregateException])
                    $failure.InnerExceptions.Count | Should -Be 2
                    $failure.InnerExceptions[0].Message | Should -Match 'compiler failure canary'
                    $failure.InnerExceptions[1].Message | Should -Match 'location failure canary'
                } elseif ($CleanupFails) {
                    $failure.Message | Should -Match 'location failure canary'
                } elseif ($Fail) {
                    $failure.Message | Should -Match 'compiler failure canary'
                } else {
                    $failure | Should -BeNullOrEmpty
                    $result | Should -Be 'controller.exe'
                }
                foreach ($name in $script:CredentialCanaries) {
                    [Environment]::GetEnvironmentVariable($name, 'Process') | Should -Be 'credential-filter-canary'
                }
                Should -Invoke cargo -Times 1 -Exactly
                Should -Invoke Pop-Location -Times 1 -Exactly
            } finally {
                foreach ($name in $saved.Keys) {
                    if ($null -eq $saved[$name]) {
                        [Environment]::SetEnvironmentVariable($name, [NullString]::Value, 'Process')
                    } else {
                        [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process')
                    }
                }
            }
        }
    }

    It 'restores an absent credential as absent rather than an empty value' {
        InModuleScope ReleaseBinaries {
            Mock Push-Location {}
            Mock Pop-Location {}
            Mock Resolve-CargoExecutable { 'controller.exe' }
            Mock cargo { 'fixture artifact' }
            $saved = [Environment]::GetEnvironmentVariable('CARGO_REGISTRY_TOKEN', 'Process')
            try {
                [Environment]::SetEnvironmentVariable('CARGO_REGISTRY_TOKEN', [NullString]::Value, 'Process')
                Get-ReleaseBinariesExecutable | Should -Be 'controller.exe'
                $null -eq [Environment]::GetEnvironmentVariable('CARGO_REGISTRY_TOKEN', 'Process') | Should -BeTrue
            } finally {
                if ($null -eq $saved) {
                    [Environment]::SetEnvironmentVariable('CARGO_REGISTRY_TOKEN', [NullString]::Value, 'Process')
                } else {
                    [Environment]::SetEnvironmentVariable('CARGO_REGISTRY_TOKEN', $saved, 'Process')
                }
            }
        }
    }
}

Describe 'Release controller input-file finalization' {
    It 'propagates a failing native exit while removing its private input' {
        InModuleScope ReleaseBinaries {
            Mock Get-ReleaseBinariesExecutable { 'Invoke-ControllerFixture' }
            Mock New-TemporaryFile { [pscustomobject]@{ FullName = 'input-fixture.json' } }
            Mock Set-Content {}
            Mock Remove-Item {}
            Mock Invoke-ControllerFixture {
                # Any nonzero native status must remain a failed helper invocation.
                $global:LASTEXITCODE = 7
                'partial output'
            }
            { Invoke-ReleaseBinariesHelper -Operation plan -InputJson '{}' -Repository 'owner/repo' } |
                Should -Throw
            Should -Invoke Remove-Item -Times 1 -Exactly
        }
    }

    It 'retains controller and input cleanup failures: operation=<Fail>, cleanup=<CleanupFails>' -ForEach @(
        @{ Fail = $false; CleanupFails = $false }
        @{ Fail = $true; CleanupFails = $false }
        @{ Fail = $false; CleanupFails = $true }
        @{ Fail = $true; CleanupFails = $true }
    ) {
        InModuleScope ReleaseBinaries -Parameters @{ Fail = $Fail; CleanupFails = $CleanupFails } {
            param($Fail, $CleanupFails)
            Mock Get-ReleaseBinariesExecutable { 'Invoke-ControllerFixture' }
            Mock New-TemporaryFile { [pscustomobject]@{ FullName = 'input-fixture.json' } }
            Mock Set-Content {}
            Mock Invoke-ControllerFixture {
                if ($Fail) { throw 'controller failure canary' }
                $global:LASTEXITCODE = 0
                '[]'
            }
            Mock Remove-Item {
                if ($CleanupFails) { throw 'input cleanup canary' }
            }
            $failure = $null
            try {
                $result = Invoke-ReleaseBinariesHelper -Operation plan -InputJson '{}' -Repository 'owner/repo'
            } catch { $failure = $_.Exception }
            if ($Fail -and $CleanupFails) {
                $failure | Should -BeOfType ([AggregateException])
                $failure.InnerExceptions.Count | Should -Be 2
                $failure.InnerExceptions[0].Message | Should -Match 'controller failure canary'
                $failure.InnerExceptions[1].Message | Should -Match 'input cleanup canary'
            } elseif ($CleanupFails) {
                $failure.Message | Should -Match 'input cleanup canary'
            } elseif ($Fail) {
                $failure.Message | Should -Match 'controller failure canary'
            } else {
                $failure | Should -BeNullOrEmpty
                $result | Should -Be '[]'
            }
            Should -Invoke Remove-Item -Times 1 -Exactly -ParameterFilter {
                $LiteralPath -ceq 'input-fixture.json'
            }
        }
    }
}
