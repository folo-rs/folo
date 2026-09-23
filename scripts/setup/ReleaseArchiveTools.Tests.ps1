#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Exercises prerequisite selection without modifying the machine or downloading packages.
BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'ReleaseArchiveTools.psm1') -Force
    InModuleScope ReleaseArchiveTools {
        function script:zip {}
        function script:unzip {}
    }
}

Describe 'Release archive prerequisite setup' {
    BeforeEach {
        Mock zip -ModuleName ReleaseArchiveTools { 'zip fixture' }
        Mock unzip -ModuleName ReleaseArchiveTools { 'unzip fixture' }
        Mock Get-ArchivePlatform -ModuleName ReleaseArchiveTools { 'linux' }
        Mock Get-Command -ModuleName ReleaseArchiveTools { [pscustomobject]@{ Source = 'fixture' } }
        Mock Invoke-ArchivePackageInstall -ModuleName ReleaseArchiveTools { }
    }

    It 'verifies installed Unix tools without invoking a package manager' {
        Install-ReleaseArchiveTool
        Should -Invoke Invoke-ArchivePackageInstall -ModuleName ReleaseArchiveTools -Times 0 -Exactly
        Should -Invoke zip -ModuleName ReleaseArchiveTools -Times 1 -Exactly
        Should -Invoke unzip -ModuleName ReleaseArchiveTools -Times 1 -Exactly
    }

    It 'installs a missing archive tool on <Platform> and verifies both executables' -ForEach @(
        @{ Platform = 'linux' }, @{ Platform = 'macos' }
    ) {
        Mock Get-ArchivePlatform -ModuleName ReleaseArchiveTools { $Platform }
        Mock Get-Command -ModuleName ReleaseArchiveTools { $null } -ParameterFilter { $Name -contains 'zip' }
        Install-ReleaseArchiveTool
        Should -Invoke Invoke-ArchivePackageInstall -ModuleName ReleaseArchiveTools -Times 1 -Exactly -ParameterFilter {
            $Platform -in @('linux', 'macos')
        }
        Should -Invoke zip -ModuleName ReleaseArchiveTools -Times 1 -Exactly
        Should -Invoke unzip -ModuleName ReleaseArchiveTools -Times 1 -Exactly
    }

    It 'does not continue after failed prerequisite installation' {
        Mock Get-Command -ModuleName ReleaseArchiveTools { $null }
        Mock Invoke-ArchivePackageInstall -ModuleName ReleaseArchiveTools { throw 'package failure canary' }
        { Install-ReleaseArchiveTool } | Should -Throw '*package failure canary*'
        Should -Invoke zip -ModuleName ReleaseArchiveTools -Times 0 -Exactly
    }
}
