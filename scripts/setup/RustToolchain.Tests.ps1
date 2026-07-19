#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Pester suite for RustToolchain.psm1. The real rustup/rustc are isolated behind mocks in the
# module's scope (so nothing is installed), and Start-Sleep is mocked in the Retry module's scope so
# the install-retry backoff does not actually wait. File I/O (channel parse, GITHUB_ENV export) runs
# for real against temp files so the on-disk result is asserted.

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'RustToolchain.psm1') -Force
}

Describe 'Get-PinnedRustChannel' {
    BeforeEach {
        $script:manifest = Join-Path ([IO.Path]::GetTempPath()) ("folo-toml-" + [guid]::NewGuid())
    }
    AfterEach {
        Remove-Item $script:manifest -ErrorAction SilentlyContinue
    }

    It 'reads the channel from a rust-toolchain.toml' {
        Set-Content -Path $script:manifest -Value @('[toolchain]', 'channel = "1.2.3"', 'components = ["clippy"]') -Encoding utf8
        Get-PinnedRustChannel -ManifestPath $script:manifest | Should -Be '1.2.3'
    }

    It 'ignores whitespace around the channel key' {
        Set-Content -Path $script:manifest -Value @('[toolchain]', '  channel   =   "nightly-2024-01-01"') -Encoding utf8
        Get-PinnedRustChannel -ManifestPath $script:manifest | Should -Be 'nightly-2024-01-01'
    }

    It 'throws when no channel is present' {
        Set-Content -Path $script:manifest -Value @('[toolchain]', 'components = ["clippy"]') -Encoding utf8
        { Get-PinnedRustChannel -ManifestPath $script:manifest } | Should -Throw
    }
}

Describe 'Set-CargoEnvDefault' {
    BeforeEach {
        $script:tempEnv = Join-Path ([IO.Path]::GetTempPath()) ("folo-env-" + [guid]::NewGuid())
        Remove-Item env:FOLO_TEST_CARGO_VAR -ErrorAction SilentlyContinue
    }
    AfterEach {
        Remove-Item env:FOLO_TEST_CARGO_VAR -ErrorAction SilentlyContinue
        Remove-Item $script:tempEnv -ErrorAction SilentlyContinue
    }

    It 'appends Name=Value to GITHUB_ENV when the variable is unset' {
        Set-CargoEnvDefault -Name 'FOLO_TEST_CARGO_VAR' -Value 'yes' -GitHubEnvPath $script:tempEnv
        Get-Content -Path $script:tempEnv | Should -Contain 'FOLO_TEST_CARGO_VAR=yes'
    }

    It 'leaves GITHUB_ENV untouched when the variable is already set' {
        $env:FOLO_TEST_CARGO_VAR = 'preset'
        Set-CargoEnvDefault -Name 'FOLO_TEST_CARGO_VAR' -Value 'yes' -GitHubEnvPath $script:tempEnv
        Test-Path $script:tempEnv | Should -BeFalse
    }

    It 'does not throw when the GITHUB_ENV path is empty' {
        { Set-CargoEnvDefault -Name 'FOLO_TEST_CARGO_VAR' -Value 'yes' -GitHubEnvPath '' } | Should -Not -Throw
    }
}

Describe 'Install-RustupToolchain' {
    BeforeEach {
        $script:savedPermitCopyRename = $env:RUSTUP_PERMIT_COPY_RENAME
        Mock rustup -ModuleName RustToolchain { $global:LASTEXITCODE = 0 }
        Mock Start-Sleep -ModuleName Retry { }
    }
    AfterEach {
        if ($null -ne $script:savedPermitCopyRename) {
            $env:RUSTUP_PERMIT_COPY_RENAME = $script:savedPermitCopyRename
        } else {
            Remove-Item env:RUSTUP_PERMIT_COPY_RENAME -ErrorAction SilentlyContinue
        }
    }

    It 'lets rustup resolve the active manifest toolchain when the channel is omitted' {
        Install-RustupToolchain
        Should -Invoke rustup -ModuleName RustToolchain -Times 1 -Exactly -ParameterFilter {
            ($args -join ' ') -eq 'toolchain install --no-self-update'
        }
    }

    It 'passes an exact channel, profile, and requested components' {
        Install-RustupToolchain -Channel 'nightly-2026-01-01' -InstallProfile minimal `
            -Component @('miri', 'rust-src')
        Should -Invoke rustup -ModuleName RustToolchain -Times 1 -Exactly -ParameterFilter {
            ($args -join ' ') -eq (
                'toolchain install nightly-2026-01-01 --profile minimal ' +
                '--component miri --component rust-src --no-self-update'
            )
        }
    }

    It 'retries one toolchain install until it succeeds' {
        $script:installAttempts = 0
        Mock rustup -ModuleName RustToolchain {
            $script:installAttempts++
            $global:LASTEXITCODE = if ($script:installAttempts -lt 2) { 1 } else { 0 }
        }

        Install-RustupToolchain -Channel '1.2.3'

        Should -Invoke rustup -ModuleName RustToolchain -Times 2 -Exactly
    }

    It 'throws after exhausting retries for one toolchain install' {
        Mock rustup -ModuleName RustToolchain { $global:LASTEXITCODE = 1 }

        { Install-RustupToolchain -Channel '1.2.3' } | Should -Throw

        Should -Invoke rustup -ModuleName RustToolchain -Times 4 -Exactly
    }
}

Describe 'Install-RustToolchain' {
    BeforeEach {
        $script:tempDir = Join-Path ([IO.Path]::GetTempPath()) ("folo-toolchain-" + [guid]::NewGuid())
        New-Item -ItemType Directory -Path $script:tempDir | Out-Null
        $script:manifest = Join-Path $script:tempDir 'rust-toolchain.toml'
        Set-Content -Path $script:manifest -Value @('[toolchain]', 'channel = "1.2.3"') -Encoding utf8
        $script:githubEnv = Join-Path $script:tempDir 'github_env'

        # Preserve and clear the Cargo env the function exports so the unset path is exercised.
        $script:savedIncremental = $env:CARGO_INCREMENTAL
        $script:savedColor = $env:CARGO_TERM_COLOR
        Remove-Item env:CARGO_INCREMENTAL -ErrorAction SilentlyContinue
        Remove-Item env:CARGO_TERM_COLOR -ErrorAction SilentlyContinue

        Mock rustup -ModuleName RustToolchain { $global:LASTEXITCODE = 0 }
        Mock rustc -ModuleName RustToolchain { $global:LASTEXITCODE = 0 }
        Mock Start-Sleep -ModuleName Retry { }
    }
    AfterEach {
        if ($null -ne $script:savedIncremental) { $env:CARGO_INCREMENTAL = $script:savedIncremental } else { Remove-Item env:CARGO_INCREMENTAL -ErrorAction SilentlyContinue }
        if ($null -ne $script:savedColor) { $env:CARGO_TERM_COLOR = $script:savedColor } else { Remove-Item env:CARGO_TERM_COLOR -ErrorAction SilentlyContinue }
        Remove-Item env:RUSTUP_PERMIT_COPY_RENAME -ErrorAction SilentlyContinue
        Remove-Item $script:tempDir -Recurse -Force -ErrorAction SilentlyContinue
    }

    It 'installs the pinned channel with the minimal profile and no self-update' {
        Install-RustToolchain -ManifestPath $script:manifest -GitHubEnvPath $script:githubEnv
        Should -Invoke rustup -ModuleName RustToolchain -Times 1 -Exactly -ParameterFilter {
            ($args -contains 'toolchain') -and ($args -contains 'install') -and ($args -contains '1.2.3') -and
            ($args -contains '--profile') -and ($args -contains 'minimal') -and ($args -contains '--no-self-update')
        }
    }

    It 'sets the pinned channel as the default' {
        Install-RustToolchain -ManifestPath $script:manifest -GitHubEnvPath $script:githubEnv
        Should -Invoke rustup -ModuleName RustToolchain -Times 1 -Exactly -ParameterFilter {
            ($args -contains 'default') -and ($args -contains '1.2.3')
        }
    }

    It 'exports CARGO_INCREMENTAL and CARGO_TERM_COLOR when they are unset' {
        Install-RustToolchain -ManifestPath $script:manifest -GitHubEnvPath $script:githubEnv
        $content = Get-Content -Path $script:githubEnv
        $content | Should -Contain 'CARGO_INCREMENTAL=0'
        $content | Should -Contain 'CARGO_TERM_COLOR=always'
    }

    It 'does not overwrite CARGO_TERM_COLOR when it is already set' {
        $env:CARGO_TERM_COLOR = 'never'
        Install-RustToolchain -ManifestPath $script:manifest -GitHubEnvPath $script:githubEnv
        Get-Content -Path $script:githubEnv | Should -Not -Contain 'CARGO_TERM_COLOR=always'
    }

    It 'runs rustc verbosely to surface the resolved compiler' {
        Install-RustToolchain -ManifestPath $script:manifest -GitHubEnvPath $script:githubEnv
        Should -Invoke rustc -ModuleName RustToolchain -Times 1 -Exactly -ParameterFilter { $args -contains '--verbose' }
    }

    It 'retries the install on a transient failure and then succeeds' {
        $script:installAttempts = 0
        Mock rustup -ModuleName RustToolchain -ParameterFilter { $args -contains 'install' } {
            $script:installAttempts++
            $global:LASTEXITCODE = if ($script:installAttempts -lt 2) { 1 } else { 0 }
        }
        Install-RustToolchain -ManifestPath $script:manifest -GitHubEnvPath $script:githubEnv
        Should -Invoke rustup -ModuleName RustToolchain -Times 2 -Exactly -ParameterFilter { $args -contains 'install' }
    }

    It 'throws when the install keeps failing after every attempt' {
        Mock rustup -ModuleName RustToolchain -ParameterFilter { $args -contains 'install' } { $global:LASTEXITCODE = 1 }
        { Install-RustToolchain -ManifestPath $script:manifest -GitHubEnvPath $script:githubEnv } | Should -Throw
        Should -Invoke rustup -ModuleName RustToolchain -Times 4 -Exactly -ParameterFilter { $args -contains 'install' }
    }

    It 'does not fail when rustup default returns a non-zero exit' {
        Mock rustup -ModuleName RustToolchain -ParameterFilter { $args -contains 'default' } { $global:LASTEXITCODE = 1 }
        { Install-RustToolchain -ManifestPath $script:manifest -GitHubEnvPath $script:githubEnv } | Should -Not -Throw
        Should -Invoke rustc -ModuleName RustToolchain -Times 1 -Exactly
    }
}
