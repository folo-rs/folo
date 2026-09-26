#requires -Version 7.6
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
        InModuleScope ReleaseArchiveTools {
            Mock Get-FileHash {
                [pscustomobject]@{ Hash = $script:SevenZipPayload.X64.Hash }
            }
        }
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

    It 'invokes the first native executable when PATH contains multiple copies' {
        Mock Get-Command -ModuleName ReleaseArchiveTools {
            [pscustomobject]@{ Source = $Name[0] }
            [pscustomobject]@{ Source = 'second-copy-must-not-run' }
        }
        Install-ReleaseArchiveTool
        Should -Invoke zip -ModuleName ReleaseArchiveTools -Times 1 -Exactly
        Should -Invoke unzip -ModuleName ReleaseArchiveTools -Times 1 -Exactly
        Should -Invoke Invoke-ArchivePackageInstall -ModuleName ReleaseArchiveTools -Times 0 -Exactly
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
                # Any nonzero native failure is representative; the exact code is incidental.
                $global:LASTEXITCODE = 2
                "7-Zip (a) $script:SevenZipVersion fixture"
            }
            $previousPreference = $PSNativeCommandUseErrorActionPreference
            Test-StandaloneSevenZip -Path Invoke-SevenZipProbeFixture `
                -ExpectedHash $script:SevenZipPayload.X64.Hash | Should -BeFalse
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
                # The rejection case needs any nonmatching marker, not another real release.
                $version = if ($Matching) { $script:SevenZipVersion } else { 'old' }
                "7-Zip (a) $version fixture"
            }
            Test-StandaloneSevenZip -Path Invoke-SevenZipProbeFixture `
                -ExpectedHash $script:SevenZipPayload.X64.Hash | Should -Be $Matching
        }
    }

    It 'rejects same-version cached bytes before executing the native probe' {
        InModuleScope ReleaseArchiveTools {
            # Preserve SHA-256 shape while deliberately differing from the selected payload.
            Mock Get-FileHash { [pscustomobject]@{ Hash = '0' * 64 } }
            Mock Invoke-SevenZipProbeFixture {
                $global:LASTEXITCODE = 0
                "7-Zip (a) $script:SevenZipVersion fixture"
            }
            Test-StandaloneSevenZip -Path Invoke-SevenZipProbeFixture `
                -ExpectedHash $script:SevenZipPayload.X64.Hash | Should -BeFalse
            Should -Invoke Invoke-SevenZipProbeFixture -Times 0 -Exactly
        }
    }

    It 'rejects a cached executable for a different native architecture' {
        InModuleScope ReleaseArchiveTools {
            Mock Invoke-SevenZipProbeFixture { throw 'wrong architecture must not run' }
            Test-StandaloneSevenZip -Path Invoke-SevenZipProbeFixture `
                -ExpectedHash $script:SevenZipPayload.Arm64.Hash | Should -BeFalse
            Should -Invoke Invoke-SevenZipProbeFixture -Times 0 -Exactly
        }
    }

    It 'reuses a validated native cache without downloading or extracting' {
        InModuleScope ReleaseArchiveTools {
            Mock Test-Path { $true }
            Mock Test-StandaloneSevenZip { $true }
            Mock Invoke-WebRequest {}
            Mock Get-Command {}
            Install-StandaloneSevenZip -Destination 'managed-tools' -Architecture Arm64
            Should -Invoke Test-StandaloneSevenZip -Times 1 -Exactly -ParameterFilter {
                $ExpectedHash -ceq $script:SevenZipPayload.Arm64.Hash
            }
            Should -Invoke Invoke-WebRequest -Times 0 -Exactly
            Should -Invoke Get-Command -Times 0 -Exactly
        }
    }

    It 'rejects unsupported Windows process architectures before acquisition' {
        InModuleScope ReleaseArchiveTools {
            Mock Test-Path {}
            { Install-StandaloneSevenZip -Destination 'managed-tools' -Architecture X86 } | Should -Throw
            Should -Invoke Test-Path -Times 0 -Exactly
        }
    }

    It 'does not continue after failed prerequisite installation' {
        Mock Get-Command -ModuleName ReleaseArchiveTools { $null }
        Mock Invoke-ArchivePackageInstall -ModuleName ReleaseArchiveTools { throw 'package failure canary' }
        { Install-ReleaseArchiveTool } | Should -Throw '*package failure canary*'
        Should -Invoke zip -ModuleName ReleaseArchiveTools -Times 0 -Exactly
    }

    It 'verifies archive and copied <Architecture> bytes before execution; archive mismatch=<ArchiveMismatch>; payload mismatch=<PayloadMismatch>' -ForEach @(
        @{ ArchiveMismatch = $false; PayloadMismatch = $false; Architecture = 'X64'; Directory = 'x64' },
        @{ ArchiveMismatch = $true; PayloadMismatch = $false; Architecture = 'X64'; Directory = 'x64' },
        @{ ArchiveMismatch = $false; PayloadMismatch = $true; Architecture = 'X64'; Directory = 'x64' },
        @{ ArchiveMismatch = $false; PayloadMismatch = $false; Architecture = 'Arm64'; Directory = 'arm64' },
        @{ ArchiveMismatch = $true; PayloadMismatch = $false; Architecture = 'Arm64'; Directory = 'arm64' },
        @{ ArchiveMismatch = $false; PayloadMismatch = $true; Architecture = 'Arm64'; Directory = 'arm64' }
    ) {
        InModuleScope ReleaseArchiveTools -Parameters @{
            ArchiveMismatch = $ArchiveMismatch; PayloadMismatch = $PayloadMismatch
            Architecture = $Architecture; Directory = $Directory
        } {
            param($ArchiveMismatch, $PayloadMismatch, $Architecture, $Directory)
            $expectedMember = Join-Path $Directory '7za.exe'
            Mock Test-Path { $false }
            Mock Join-Path { 'Invoke-SevenZipProbeFixture' } -ParameterFilter {
                $Path -eq 'managed-tools' -and $ChildPath -eq '7za.exe'
            }
            Mock New-Item {}
            Mock Invoke-WebRequest {}
            Mock Get-FileHash {
                $isPayload = $LiteralPath -eq 'Invoke-SevenZipProbeFixture'
                $mismatch = if ($isPayload) { $PayloadMismatch } else { $ArchiveMismatch }
                $expected = if ($isPayload) { $script:SevenZipPayload[$Architecture.ToString()].Hash } else { $script:SevenZipHash }
                [pscustomobject]@{ Hash = $(if ($mismatch) { '0' * 64 } else { $expected }) }
            }
            Mock Invoke-WithRetry { & $Action }
            Mock Copy-Item {}
            Mock Remove-Item {}
            Mock Invoke-ArchiveExtractorFixture {}
            Mock Invoke-SevenZipProbeFixture {
                $global:LASTEXITCODE = 0
                "7-Zip (a) $script:SevenZipVersion fixture"
            }
            Mock Get-Command { [pscustomobject]@{ Source = 'Invoke-ArchiveExtractorFixture' } }

            $previousRoot = $env:SystemRoot
            try {
                # The mocked application boundary makes this Windows bootstrap policy test
                # portable, without creating files or downloading an executable.
                $env:SystemRoot = [IO.Path]::GetTempPath()
                if ($ArchiveMismatch) {
                    { Install-StandaloneSevenZip -Destination 'managed-tools' -Architecture $Architecture } | Should -Throw
                    Should -Invoke Invoke-ArchiveExtractorFixture -Times 0 -Exactly
                    Should -Invoke Copy-Item -Times 0 -Exactly
                } else {
                    if ($PayloadMismatch) {
                        { Install-StandaloneSevenZip -Destination 'managed-tools' -Architecture $Architecture } | Should -Throw
                    } else {
                        Install-StandaloneSevenZip -Destination 'managed-tools' -Architecture $Architecture
                    }
                    Should -Invoke Invoke-ArchiveExtractorFixture -Times 1 -Exactly
                    Should -Invoke Copy-Item -Times 2 -Exactly
                    Should -Invoke Copy-Item -Times 1 -Exactly -ParameterFilter {
                        $LiteralPath.EndsWith($expectedMember, [StringComparison]::Ordinal)
                    }
                }
                $probeCount = if ($ArchiveMismatch -or $PayloadMismatch) { 0 } else { 1 }
                Should -Invoke Invoke-SevenZipProbeFixture -Times $probeCount -Exactly
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
