#Requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

# Pester suite for ReleaseAutomation.psm1. Where it is safe on fixtures, the tests drive the
# real external tools: workspace discovery runs actual `cargo metadata` against
# a fixture workspace, and Set-GitHubOutput performs real file I/O. The tools that would touch
# crates.io / GitHub for real -- `release-plz` and `gh` -- are isolated behind functions the
# tests mock in the module's scope.

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'ReleaseAutomation.psm1') -Force

    $script:FixtureDir = Join-Path $PSScriptRoot 'fixtures'
    $script:MetadataManifest = Join-Path $script:FixtureDir 'metadata-workspace/Cargo.toml'
    $script:MultiBinaryManifest = Join-Path $script:FixtureDir 'multi-binary-workspace/Cargo.toml'
}

Describe 'Get-PublishableBinaryCrate (real cargo metadata on a fixture workspace)' {
    BeforeAll {
        $script:Crates = Get-PublishableBinaryCrate -ManifestPath $script:MetadataManifest
    }

    It 'selects publishable crates that own a bin target' {
        $script:Crates.Name | Should -Contain 'pub-bin'
        $script:Crates.Name | Should -Contain 'demo-tool'
    }

    It 'excludes library-only crates' {
        $script:Crates.Name | Should -Not -Contain 'pub-lib'
        $script:Crates.Name | Should -Not -Contain 'demo-tool-core'
    }

    It 'excludes non-publishable crates even when they own a bin target' {
        $script:Crates.Name | Should -Not -Contain 'nopub-bin'
    }

    It 'reports the manifest version for each selected crate' {
        ($script:Crates | Where-Object Name -EQ 'demo-tool').Version | Should -Be '2.3.4'
        ($script:Crates | Where-Object Name -EQ 'pub-bin').Version | Should -Be '0.1.0'
    }

    It 'reports the actual binary target name' {
        ($script:Crates | Where-Object Name -EQ 'demo-tool').Binary | Should -Be 'demo-bin'
    }

    It 'returns crates sorted by name' {
        $script:Crates.Name | Should -Be @('demo-tool', 'pub-bin', 'win-tool')
    }

    It 'projects a declared release-target restriction onto the crate' {
        ($script:Crates | Where-Object Name -EQ 'win-tool').ReleaseTargets |
            Should -Be @('x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc')
    }

    It 'leaves the restriction empty for a crate that declares none' {
        @(($script:Crates | Where-Object Name -EQ 'pub-bin').ReleaseTargets).Count | Should -Be 0
    }

    It 'rejects a publishable package with several binary targets' {
        { Get-PublishableBinaryCrate -ManifestPath $script:MultiBinaryManifest } | Should -Throw
    }

    It 'does not add Git tracking to binary publication discovery' {
        Mock git -ModuleName ReleaseAutomation {
            throw 'binary publication discovery must not query Git tracking'
        }

        $crate = Get-PublishableBinaryCrate -ManifestPath $script:MetadataManifest

        $crate.Name | Should -Contain 'pub-bin'
        Should -Invoke git -ModuleName ReleaseAutomation -Times 0 -Exactly
    }
}

Describe 'Get-DeclaredReleaseTarget (crafted package objects)' {
    It 'returns nothing for a package with no metadata field at all' {
        @(Get-DeclaredReleaseTarget -Package ([pscustomobject]@{ name = 'crafted' })).Count | Should -Be 0
    }

    It 'returns nothing for a package whose metadata has no release-plan section' {
        $pkg = [pscustomobject]@{
            name     = 'crafted'
            metadata = [pscustomobject]@{ binstall = [pscustomobject]@{ 'pkg-fmt' = 'zip' } }
        }
        @(Get-DeclaredReleaseTarget -Package $pkg).Count | Should -Be 0
    }

    It 'returns nothing for a release-plan section that declares no release targets' {
        $pkg = [pscustomobject]@{
            name     = 'crafted'
            metadata = [pscustomobject]@{ 'release-plan' = [pscustomobject]@{ 'something-else' = 'value' } }
        }
        @(Get-DeclaredReleaseTarget -Package $pkg).Count | Should -Be 0
    }

    It 'yields one triple per declared target, unrolled by the pipeline' {
        $pkg = [pscustomobject]@{
            name     = 'crafted'
            metadata = [pscustomobject]@{
                'release-plan' = [pscustomobject]@{ 'release-targets' = @('x86_64-pc-windows-msvc') }
            }
        }
        $declared = @(Get-DeclaredReleaseTarget -Package $pkg)
        $declared.Count | Should -Be 1
        $declared[0] | Should -Be 'x86_64-pc-windows-msvc'
    }
}

Describe 'Get-PublishableCrate (real cargo metadata on a fixture workspace)' {
    BeforeAll {
        $script:AllCrates = Get-PublishableCrate -ManifestPath $script:MetadataManifest
    }

    It 'includes publishable crates regardless of target kind (libraries and binaries)' {
        $script:AllCrates.Name | Should -Contain 'pub-bin'
        $script:AllCrates.Name | Should -Contain 'pub-lib'
        $script:AllCrates.Name | Should -Contain 'demo-tool'
        $script:AllCrates.Name | Should -Contain 'demo-tool-core'
    }

    It 'excludes non-publishable crates' {
        $script:AllCrates.Name | Should -Not -Contain 'nopub-bin'
    }

    It 'returns crates sorted by name with versions' {
        $script:AllCrates.Name | Should -Be @('demo-tool', 'demo-tool-core', 'pub-bin', 'pub-lib', 'win-tool')
        ($script:AllCrates | Where-Object Name -EQ 'demo-tool').Version | Should -Be '2.3.4'
    }

    It 'does not add Git tracking to workspace publication discovery' {
        Mock git -ModuleName ReleaseAutomation {
            throw 'publication discovery must not query Git tracking'
        }

        $crate = Get-PublishableCrate -ManifestPath $script:MetadataManifest

        $crate.Name | Should -Contain 'pub-lib'
        Should -Invoke git -ModuleName ReleaseAutomation -Times 0 -Exactly
    }
}

Describe 'Get-ReleaseTarget' {
    BeforeAll {
        $script:Targets = Get-ReleaseTarget
    }

    It 'maps each triple to its runner' {
        ($script:Targets | Where-Object Triple -EQ 'x86_64-unknown-linux-gnu').Os | Should -Be 'ubuntu-latest'
        ($script:Targets | Where-Object Triple -EQ 'aarch64-unknown-linux-gnu').Os | Should -Be 'ubuntu-24.04-arm'
        ($script:Targets | Where-Object Triple -EQ 'x86_64-pc-windows-msvc').Os | Should -Be 'windows-latest'
        ($script:Targets | Where-Object Triple -EQ 'aarch64-pc-windows-msvc').Os | Should -Be 'windows-11-arm'
        ($script:Targets | Where-Object Triple -EQ 'aarch64-apple-darwin').Os | Should -Be 'macos-latest'
    }

    It 'builds Apple Silicon but not Intel macOS' {
        $script:Targets.Triple | Should -Contain 'aarch64-apple-darwin'
        $script:Targets.Triple | Should -Not -Contain 'x86_64-apple-darwin'
    }
}

Describe 'Get-BinaryReleaseAsset for release existence reconciliation' {
    It 'distinguishes a missing release from an existing empty release' {
        Mock gh -ModuleName ReleaseAutomation {
            $global:LASTEXITCODE = 1
            'release not found'
        }
        $missing = Get-BinaryReleaseAsset -Tag 'app-v1.0.0'
        $null -eq $missing | Should -BeTrue
        Mock gh -ModuleName ReleaseAutomation {
            $global:LASTEXITCODE = 0
            '{"assets":[]}'
        }
        $assets = Get-BinaryReleaseAsset -Tag 'app-v1.0.0'
        ($assets -is [array]) | Should -BeTrue
        $assets.Count | Should -Be 0
    }

    It 'does not interpret authentication or network failure as a missing release' {
        Mock gh -ModuleName ReleaseAutomation {
            $global:LASTEXITCODE = 1
            'HTTP 503: Service Unavailable'
        }
        { Get-BinaryReleaseAsset -Tag 'app-v1.0.0' } | Should -Throw '*503*'
    }
}

Describe 'Invoke-ReleasePublish (mocked release-plz)' {
    BeforeEach {
        # These warnings describe injected fixture failures, so assert their calls rather than
        # printing them as unexamined validation warnings.
        Mock Write-Warning -ModuleName Retry {}
    }

    It 'invokes release-plz once with the registry-only config on success' {
        $configPath = Join-Path $TestDrive 'ci.toml'
        Mock release-plz -ModuleName ReleaseAutomation { $global:LASTEXITCODE = 0 }
        Invoke-ReleasePublish -ConfigPath $configPath -Attempt 3 -DelaySeconds 0
        Should -Invoke release-plz -ModuleName ReleaseAutomation -Times 1 -Exactly `
            -ParameterFilter { ($args -contains 'release') -and ($args -contains '--config') -and ($args -contains $configPath) }
    }

    It 'retries on a non-zero exit and then succeeds' {
        $configPath = Join-Path $TestDrive 'ci.toml'
        $script:attempts = 0
        Mock release-plz -ModuleName ReleaseAutomation {
            $script:attempts++
            $global:LASTEXITCODE = if ($script:attempts -lt 2) { 1 } else { 0 }
        }
        Invoke-ReleasePublish -ConfigPath $configPath -Attempt 3 -DelaySeconds 0 `
            -WarningAction SilentlyContinue
        Should -Invoke Write-Warning -ModuleName Retry -Times 1 -Exactly
        Should -Invoke release-plz -ModuleName ReleaseAutomation -Times 2 -Exactly
    }

    It 'throws after every attempt fails' {
        $configPath = Join-Path $TestDrive 'ci.toml'
        Mock release-plz -ModuleName ReleaseAutomation { $global:LASTEXITCODE = 1 }
        {
            Invoke-ReleasePublish -ConfigPath $configPath -Attempt 3 -DelaySeconds 0 `
                -WarningAction SilentlyContinue
        } | Should -Throw
        Should -Invoke release-plz -ModuleName ReleaseAutomation -Times 3 -Exactly
        Should -Invoke Write-Warning -ModuleName Retry -Times 2 -Exactly
    }
}

Describe 'Set-GitHubOutput' {
    It 'appends name=value to the GITHUB_OUTPUT file when it is set' {
        $original = $env:GITHUB_OUTPUT
        $file = Join-Path $TestDrive ("out-" + [guid]::NewGuid())
        try {
            $env:GITHUB_OUTPUT = $file
            Set-GitHubOutput -Name 'matrix' -Value '[]'
            Set-GitHubOutput -Name 'has_binaries' -Value 'false'
            $lines = Get-Content $file
            $lines | Should -Contain 'matrix=[]'
            $lines | Should -Contain 'has_binaries=false'
        } finally {
            if ($null -ne $original) { $env:GITHUB_OUTPUT = $original } else { Remove-Item Env:GITHUB_OUTPUT -ErrorAction SilentlyContinue }
            if (Test-Path $file) { Remove-Item $file -Force }
        }
    }

    It 'does not throw when GITHUB_OUTPUT is unset' {
        $original = $env:GITHUB_OUTPUT
        try {
            Remove-Item Env:GITHUB_OUTPUT -ErrorAction SilentlyContinue
            { Set-GitHubOutput -Name 'matrix' -Value '[]' } | Should -Not -Throw
        } finally {
            if ($null -ne $original) { $env:GITHUB_OUTPUT = $original }
        }
    }

    It 'rejects an empty release-asset output before writing it' {
        $original = $env:GITHUB_OUTPUT
        $file = Join-Path $TestDrive 'empty-output'
        New-Item -ItemType File -Path $file | Out-Null
        try {
            $env:GITHUB_OUTPUT = $file
            { Set-GitHubOutput -Name 'matrix' -Value '' } |
                Should -Throw "*GitHub output 'matrix' must not be empty*"
            @(Get-Content -LiteralPath $file).Count | Should -Be 0
        } finally {
            if ($null -ne $original) {
                $env:GITHUB_OUTPUT = $original
            } else {
                Remove-Item Env:GITHUB_OUTPUT -ErrorAction SilentlyContinue
            }
        }
    }
}
