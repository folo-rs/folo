#Requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Exercises the shared local/CI installer without network access or global installation.
# Archive/cleanup cases are integration tests against TestDrive; subprocesses are mocked.
# Ref: docs/build-and-tooling.md#development-tool-installation.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'CargoTools.psm1') -Force
}

Describe 'Cargo installation root' {
    BeforeEach {
        $script:savedCargoHome = $env:CARGO_HOME
        $script:savedInstallRoot = $env:CARGO_INSTALL_ROOT
        $env:CARGO_HOME = $null
        $env:CARGO_INSTALL_ROOT = $null
    }
    AfterEach {
        $env:CARGO_HOME = $script:savedCargoHome
        $env:CARGO_INSTALL_ROOT = $script:savedInstallRoot
    }

    It 'defaults to the home Cargo directory' {
        Get-CargoInstallRoot | Should -Be (Join-Path $HOME '.cargo')
    }

    It 'honors CARGO_HOME and gives CARGO_INSTALL_ROOT precedence' {
        $env:CARGO_HOME = Join-Path $TestDrive 'cargo home'
        Get-CargoInstallRoot | Should -Be $env:CARGO_HOME
        $env:CARGO_INSTALL_ROOT = Join-Path $TestDrive 'install root'
        Get-CargoInstallRoot | Should -Be $env:CARGO_INSTALL_ROOT
    }
}

Describe 'Bootstrap selection' {
    It 'rejects unsupported architectures before downloading' {
        InModuleScope CargoTools {
            { Get-BinstallBuild -Platform windows -Architecture X86 } | Should -Throw
        }
    }

    It 'selects native archives for <Platform>/<Architecture>' -ForEach @(
        @{ Platform = 'windows'; Architecture = 'X64'; Target = 'x86_64-pc-windows-msvc'; Format = 'zip' }
        @{ Platform = 'windows'; Architecture = 'Arm64'; Target = 'aarch64-pc-windows-msvc'; Format = 'zip' }
        @{ Platform = 'linux'; Architecture = 'X64'; Target = 'x86_64-unknown-linux-musl'; Format = 'tgz' }
        @{ Platform = 'linux'; Architecture = 'Arm64'; Target = 'aarch64-unknown-linux-musl'; Format = 'tgz' }
        @{ Platform = 'mac'; Architecture = 'X64'; Target = 'x86_64-apple-darwin'; Format = 'zip' }
        @{ Platform = 'mac'; Architecture = 'Arm64'; Target = 'aarch64-apple-darwin'; Format = 'zip' }
    ) {
        InModuleScope CargoTools -Parameters @{ Platform = $Platform; Architecture = $Architecture; Target = $Target; Format = $Format } {
            $build = Get-BinstallBuild -Platform $Platform -Architecture $Architecture
            $build.Target | Should -Be $Target
            $build.Format | Should -Be $Format
        }
    }

    It 'reads an exact bootstrap pin without relying on dotenv environment loading' {
        Mock Read-DotEnvFile -ModuleName CargoTools { @{ JUST_VERSION = '9.8.7' } }
        Get-BootstrapToolVersion -Name JUST | Should -Be '9.8.7'
    }

    It 'rejects missing or floating bootstrap versions' -ForEach @('', '*', '^1.2.3') {
        $script:invalidVersion = $_
        Mock Read-DotEnvFile -ModuleName CargoTools { @{ JUST_VERSION = $script:invalidVersion } }
        { Get-BootstrapToolVersion -Name JUST } | Should -Throw
    }

    It 'reads the bare version emitted by binstall -V' {
        InModuleScope CargoTools {
            function Get-VersionFixture { $global:LASTEXITCODE = 0; '9.8.7' }
            Get-BinstallVersion -Path Get-VersionFixture | Should -Be '9.8.7'
        }
    }

    It 'rejects invalid version output or an unsuccessful version command' -ForEach @(
        @{ ExitCode = 0; Output = 'unexpected output' }
        @{ ExitCode = 1; Output = '9.8.7' }
    ) {
        InModuleScope CargoTools -Parameters @{ ExitCode = $ExitCode; Output = $Output } {
            function Get-VersionFixture { $global:LASTEXITCODE = $ExitCode; $Output }
            { Get-BinstallVersion -Path Get-VersionFixture } | Should -Throw
        }
    }
}

Describe 'Verified binstall bootstrap' -Tag Integration {
    BeforeEach {
        $script:root = Join-Path $TestDrive ([guid]::NewGuid().ToString('N'))
        $script:binaryName = if ($IsWindows) { 'cargo-binstall.exe' } else { 'cargo-binstall' }
        $script:installed = Join-Path $script:root 'bin' $script:binaryName
        $fixture = New-Item -ItemType Directory -Path (Join-Path $TestDrive ([guid]::NewGuid().ToString('N')))
        Set-Content -LiteralPath (Join-Path $fixture.FullName $script:binaryName) -Value 'verified fixture'
        $script:archive = Join-Path $TestDrive "$([guid]::NewGuid()).zip"
        Compress-Archive -Path (Join-Path $fixture.FullName $script:binaryName) -DestinationPath $script:archive
        $script:digest = (Get-FileHash -LiteralPath $script:archive -Algorithm SHA256).Hash
        $script:downloadPath = $null

        Mock Get-BootstrapToolVersion -ModuleName CargoTools { '9.8.7' }
        Mock Get-BinstallBuild -ModuleName CargoTools {
            @{ Target = 'fixture-target'; Format = 'zip'; Sha256 = $script:digest }
        }
        Mock Get-BinstallVersion -ModuleName CargoTools { '9.8.7' }
        Mock Invoke-WebRequest -ModuleName CargoTools {
            param($OutFile)
            $script:downloadPath = $OutFile
            Copy-Item -LiteralPath $script:archive -Destination $OutFile
        }
        Mock Start-Sleep -ModuleName Retry {}
    }

    It 'installs only verified bytes into the selected root and cleans staging' {
        Install-CargoBinstall -Root $script:root | Should -Be $script:installed
        Get-Content -LiteralPath $script:installed | Should -Be 'verified fixture'
        Test-Path -LiteralPath (Split-Path $script:downloadPath) | Should -BeFalse
        Should -Invoke Invoke-WebRequest -ModuleName CargoTools -Times 1 -Exactly -ParameterFilter {
            $Uri -eq 'https://github.com/cargo-bins/cargo-binstall/releases/download/v9.8.7/cargo-binstall-fixture-target.zip'
        }
    }

    It 'reuses a matching installation without downloading' {
        $null = New-Item -ItemType Directory -Path (Split-Path $script:installed) -Force
        Set-Content -LiteralPath $script:installed -Value 'existing'
        Install-CargoBinstall -Root $script:root | Should -Be $script:installed
        Get-Content -LiteralPath $script:installed | Should -Be 'existing'
        Should -Invoke Invoke-WebRequest -ModuleName CargoTools -Times 0 -Exactly
        Should -Invoke Get-BinstallVersion -ModuleName CargoTools -Times 1 -Exactly -ParameterFilter {
            $Path -eq $script:installed
        }
    }

    It 'reconciles an older or newer bootstrap version: <_>' -ForEach @('1.0.0', '99.0.0') {
        $null = New-Item -ItemType Directory -Path (Split-Path $script:installed) -Force
        Set-Content -LiteralPath $script:installed -Value 'old content'
        $script:existingVersion = $_
        $script:versionReads = 0
        Mock Get-BinstallVersion -ModuleName CargoTools {
            $script:versionReads++
            if ($script:versionReads -eq 1) { $script:existingVersion } else { '9.8.7' }
        }

        Install-CargoBinstall -Root $script:root | Should -Be $script:installed
        Get-Content -LiteralPath $script:installed | Should -Be 'verified fixture'
        Should -Invoke Invoke-WebRequest -ModuleName CargoTools -Times 1 -Exactly
    }

    It 'rejects checksum mismatch before extraction or execution and cleans staging' {
        $script:digest = '0' * 64
        Mock Expand-Archive -ModuleName CargoTools { throw 'Unverified archive reached extraction.' }
        { Install-CargoBinstall -Root $script:root } | Should -Throw
        Test-Path -LiteralPath $script:installed | Should -BeFalse
        Test-Path -LiteralPath (Split-Path $script:downloadPath) | Should -BeFalse
        Should -Invoke Invoke-WebRequest -ModuleName CargoTools -Times 4 -Exactly
        Should -Invoke Expand-Archive -ModuleName CargoTools -Times 0 -Exactly
        Should -Invoke Get-BinstallVersion -ModuleName CargoTools -Times 0 -Exactly
    }

    It 'preserves the installed binary when the archive reports the wrong version' {
        $null = New-Item -ItemType Directory -Path (Split-Path $script:installed) -Force
        Set-Content -LiteralPath $script:installed -Value 'existing'
        Mock Get-BinstallVersion -ModuleName CargoTools { '1.0.0' }
        { Install-CargoBinstall -Root $script:root } | Should -Throw
        Get-Content -LiteralPath $script:installed | Should -Be 'existing'
        Test-Path -LiteralPath (Split-Path $script:downloadPath) | Should -BeFalse
    }

    It 'surfaces exhausted download failures without installing anything' {
        Mock Invoke-WebRequest -ModuleName CargoTools { throw 'Download unavailable.' }
        { Install-CargoBinstall -Root $script:root } | Should -Throw
        Test-Path -LiteralPath $script:installed | Should -BeFalse
        Should -Invoke Invoke-WebRequest -ModuleName CargoTools -Times 4 -Exactly
    }
}

Describe 'Binary-first tool orchestration' {
    BeforeEach {
        Mock Install-CargoBinstall -ModuleName CargoTools { Join-Path $Root 'bin' 'cargo-binstall' }
        Mock Invoke-CargoBinstall -ModuleName CargoTools {}
    }

    It 'preserves exact pins, locked fallback and publisher policy without forcing reinstalls' {
        $root = Join-Path $TestDrive 'tool root'
        Install-CargoTool -Root $root -Package @('first@1.2.3', 'second@=4.5.6')
        Should -Invoke Install-CargoBinstall -ModuleName CargoTools -Times 1 -Exactly -ParameterFilter {
            $Root -eq $root
        }
        Should -Invoke Invoke-CargoBinstall -ModuleName CargoTools -Times 1 -Exactly -ParameterFilter {
            $Executable -eq (Join-Path $root 'bin' 'cargo-binstall') -and
            ($ArgumentList -join '|') -eq (
                "--no-confirm|--locked|--disable-strategies|quick-install|--disable-telemetry|" +
                "--no-discover-github-token|--root|$root|first@1.2.3|second@=4.5.6"
            )
        }
    }

    It 'rejects floating specifications before bootstrapping: <_>' -ForEach @('crate', 'crate@*', 'crate@^1.2.3') {
        { Install-CargoTool -Package $_ } | Should -Throw
        Should -Invoke Install-CargoBinstall -ModuleName CargoTools -Times 0 -Exactly
    }

    It 'propagates installation failures' {
        Mock Invoke-CargoBinstall -ModuleName CargoTools { throw 'Installation failed.' }
        { Install-CargoTool -Package 'crate@1.2.3' } | Should -Throw
    }

    It 'passes a publisher URL template without changing the install policy' {
        Install-CargoTool -Package 'crate@1.2.3' -PackageUrl 'https://publisher.invalid/{ version }.zip'
        Should -Invoke Invoke-CargoBinstall -ModuleName CargoTools -Times 1 -Exactly -ParameterFilter {
            ($ArgumentList -contains '--pkg-url') -and
            ($ArgumentList -contains 'https://publisher.invalid/{ version }.zip') -and
            ($ArgumentList -contains '--locked') -and ($ArgumentList -contains 'quick-install')
        }
    }

    It 'rejects a URL override for a batch before bootstrapping' {
        { Install-CargoTool -Package @('first@1.2.3', 'second@4.5.6') -PackageUrl 'https://publisher.invalid/archive.zip' } |
            Should -Throw
        Should -Invoke Install-CargoBinstall -ModuleName CargoTools -Times 0 -Exactly
    }
}
