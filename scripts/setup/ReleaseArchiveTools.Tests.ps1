#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Exercises prerequisite selection without modifying the machine or downloading packages.
BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'ReleaseArchiveTools.psm1') -Force
    InModuleScope ReleaseArchiveTools {
        function script:zip {}
        function script:unzip {}
        function script:Invoke-ArchiveExtractorFixture {}
        function script:Invoke-SevenZipProbeFixture {}
    }
}

Describe 'Release archive prerequisite setup' {
    BeforeEach {
        Mock zip -ModuleName ReleaseArchiveTools { 'zip fixture' }
        Mock unzip -ModuleName ReleaseArchiveTools { 'unzip fixture' }
        Mock Get-ArchivePlatform -ModuleName ReleaseArchiveTools { 'linux' }
        Mock Get-Command -ModuleName ReleaseArchiveTools { [pscustomobject]@{ Source = $Name[0] } }
        Mock Invoke-ArchivePackageInstall -ModuleName ReleaseArchiveTools { }
    }

    It 'verifies installed Unix tools without invoking a package manager' {
        Install-ReleaseArchiveTool
        Should -Invoke Invoke-ArchivePackageInstall -ModuleName ReleaseArchiveTools -Times 0 -Exactly
        Should -Invoke zip -ModuleName ReleaseArchiveTools -Times 1 -Exactly
        Should -Invoke unzip -ModuleName ReleaseArchiveTools -Times 1 -Exactly
        Should -Invoke Get-Command -ModuleName ReleaseArchiveTools -Times 4 -Exactly -ParameterFilter {
            $CommandType -eq 'Application'
        }
    }

    It 'installs a missing archive tool on <Platform> and verifies both executables' -ForEach @(
        @{ Platform = 'linux' }, @{ Platform = 'macos' }
    ) {
        Mock Get-ArchivePlatform -ModuleName ReleaseArchiveTools { $Platform }
        Mock Get-Command -ModuleName ReleaseArchiveTools { $null } -ParameterFilter {
            $Name -contains 'zip' -and $ErrorAction -eq 'SilentlyContinue'
        }
        Install-ReleaseArchiveTool
        Should -Invoke Invoke-ArchivePackageInstall -ModuleName ReleaseArchiveTools -Times 1 -Exactly -ParameterFilter {
            $Platform -in @('linux', 'macos')
        }

        Should -Invoke zip -ModuleName ReleaseArchiveTools -Times 1 -Exactly
        Should -Invoke unzip -ModuleName ReleaseArchiveTools -Times 1 -Exactly
    }

    It 'rejects a failed cached version probe without masking later native failures' {
        InModuleScope ReleaseArchiveTools {
            Mock Invoke-SevenZipProbeFixture {
                $global:LASTEXITCODE = 2
                "7-Zip (a) $script:SevenZipVersion fixture"
            }
            $previousPreference = $PSNativeCommandUseErrorActionPreference
            Test-StandaloneSevenZip -Path Invoke-SevenZipProbeFixture | Should -BeFalse
            $PSNativeCommandUseErrorActionPreference | Should -Be $previousPreference
        }
    }

    It 'accepts only a successful matching cached version; matching=<Matching>' -ForEach @(
        @{ Matching = $true }, @{ Matching = $false }
    ) {
        InModuleScope ReleaseArchiveTools -Parameters @{ Matching = $Matching } {
            param($Matching)
            Mock Invoke-SevenZipProbeFixture {
                $global:LASTEXITCODE = 0
                $version = if ($Matching) { $script:SevenZipVersion } else { 'old' }
                "7-Zip (a) $version fixture"
            }
            Test-StandaloneSevenZip -Path Invoke-SevenZipProbeFixture | Should -Be $Matching
        }
    }

    It 'does not continue after failed prerequisite installation' {
        Mock Get-Command -ModuleName ReleaseArchiveTools { $null }
        Mock Invoke-ArchivePackageInstall -ModuleName ReleaseArchiveTools { throw 'package failure canary' }
        { Install-ReleaseArchiveTool } | Should -Throw '*package failure canary*'
        Should -Invoke zip -ModuleName ReleaseArchiveTools -Times 0 -Exactly
    }

    It 'uses the Windows system extractor only after a matching checksum; mismatch=<Mismatch>' -ForEach @(
        @{ Mismatch = $false }, @{ Mismatch = $true }
    ) {
        InModuleScope ReleaseArchiveTools -Parameters @{ Mismatch = $Mismatch } {
            param($Mismatch)
            Mock Test-Path { $false }
            Mock New-Item {}
            Mock Invoke-WebRequest {}
            Mock Get-FileHash {
                [pscustomobject]@{ Hash = $(if ($Mismatch) { '0' * 64 } else { $script:SevenZipHash }) }
            }
            Mock Invoke-WithRetry { & $Action }
            Mock Copy-Item {}
            Mock Remove-Item {}
            Mock Invoke-ArchiveExtractorFixture {}
            Mock Get-Command { [pscustomobject]@{ Source = 'Invoke-ArchiveExtractorFixture' } }

            $previousRoot = $env:SystemRoot
            try {
                # The mocked application boundary makes this Windows bootstrap policy test
                # portable, without creating files or downloading an executable.
                $env:SystemRoot = [IO.Path]::GetTempPath()
                if ($Mismatch) {
                    { Install-StandaloneSevenZip -Destination 'managed-tools' } | Should -Throw
                    Should -Invoke Invoke-ArchiveExtractorFixture -Times 0 -Exactly
                    Should -Invoke Copy-Item -Times 0 -Exactly
                } else {
                    Install-StandaloneSevenZip -Destination 'managed-tools'
                    Should -Invoke Invoke-ArchiveExtractorFixture -Times 1 -Exactly
                    Should -Invoke Copy-Item -Times 2 -Exactly
                }
                Should -Invoke Get-Command -Times 1 -Exactly -ParameterFilter {
                    $Name -contains (Join-Path $env:SystemRoot 'System32' 'tar.exe') -and
                    $CommandType -eq 'Application'
                }
            } finally {
                $env:SystemRoot = $previousRoot
            }
        }
    }
}
