#requires -Version 7.6

# Release-automation logic for the `Release` GitHub workflow (.github/workflows/release.yml).
#
# The workflow steps and release recipes import this module for package discovery, runner
# selection and registry publication. ReleasePublication.psm1 owns GitHub reconciliation;
# ReleaseBinaries.psm1 connects native batch planning/execution to the Rust controller.
# Pester exercises the PowerShell responsibilities against fixtures.
#
# The functions run real external tools where that is safe on fixtures (`cargo metadata`, file
# I/O) and isolate the ones that would touch crates.io / GitHub for real (`release-plz`, `gh`)
# behind explicit boundaries the tests mock.

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true

# The transient-fault retry (used by Invoke-ReleasePublish) is the shared workspace helper rather
# than a private copy, so every network-facing script retries the same way.
Import-Module (Join-Path $PSScriptRoot '..' 'utility' 'Retry.psm1') -Force

function Get-ReleaseTarget {
    # The single source of truth for the triple -> runner mapping. The workflow's build matrix
    # is derived from this (via release-binaries), so a target is added in exactly one
    # place. Native runners, one per target, no cross-compilation. GitHub offers `-latest` only
    # for x64 Linux/Windows and macOS (macos-latest is arm64); ARM Linux/Windows have no
    # `-latest` alias, so they are pinned by version. Intel macOS is intentionally absent.
    [CmdletBinding()]
    param()

    @(
        [pscustomobject]@{ Triple = 'x86_64-unknown-linux-gnu';  Os = 'ubuntu-latest' }
        [pscustomobject]@{ Triple = 'aarch64-unknown-linux-gnu'; Os = 'ubuntu-24.04-arm' }
        [pscustomobject]@{ Triple = 'x86_64-pc-windows-msvc';     Os = 'windows-latest' }
        [pscustomobject]@{ Triple = 'aarch64-pc-windows-msvc';    Os = 'windows-11-arm' }
        [pscustomobject]@{ Triple = 'aarch64-apple-darwin';       Os = 'macos-latest' }
    )
}

function Get-DeclaredReleaseTarget {
    # The target triples a crate restricts its prebuilt binaries to, read from its manifest's
    # `[package.metadata.release-plan] release-targets`. Returns an empty array when the crate declares
    # nothing, which means every target in Get-ReleaseTarget - the default, and what a portable
    # crate wants. A crate that only functions on some platforms names that subset so the workflow
    # does not publish archives whose binary could never run. Takes a `cargo metadata` package
    # object; StrictMode makes an absent property throw, so every hop is guarded explicitly.
    # PowerShell unrolls a single-element result, so callers wrap the call in @().
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][object] $Package
    )

    if ($Package.PSObject.Properties.Name -notcontains 'metadata') { return @() }
    if ($null -eq $Package.metadata) { return @() }
    if ($Package.metadata.PSObject.Properties.Name -notcontains 'release-plan') { return @() }

    $releasePlan = $Package.metadata.'release-plan'
    if ($null -eq $releasePlan) { return @() }
    if ($releasePlan.PSObject.Properties.Name -notcontains 'release-targets') { return @() }

    @($releasePlan.'release-targets')
}

function Get-BinaryTarget {
    # Returns the Cargo binary targets declared by a package metadata object. Keeping this
    # extraction in one function lets release planning and validation agree on what a binary
    # package contains.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][object] $Package
    )

    if ($Package.PSObject.Properties.Name -notcontains 'targets' -or
        $null -eq $Package.targets) {
        return @()
    }
    @($Package.targets | Where-Object { $_.kind -contains 'bin' })
}

function Get-WorkspaceMember {
    # Cargo defines the package inventory used by the legacy publisher and tag reconciliation.
    # Version assessment and tracked-input validation belong to cargo-release-plan.
    [CmdletBinding()]
    param(
        [string] $ManifestPath
    )

    $cargoArgs = @('metadata', '--no-deps', '--format-version', '1')
    if ($ManifestPath) { $cargoArgs += @('--manifest-path', $ManifestPath) }

    $configuredTargetDirectory =
        [Environment]::GetEnvironmentVariable('CARGO_TARGET_DIR', 'Process')
    try {
        # Cargo rejects an explicitly present empty value. Treat it as the absence it represents
        # for this subprocess without changing the caller's environment permanently.
        if ($null -ne $configuredTargetDirectory -and
            $configuredTargetDirectory.Length -eq 0) {
            Remove-Item Env:CARGO_TARGET_DIR
        }
        $metadata = & cargo @cargoArgs | ConvertFrom-Json
    } finally {
        if ($null -ne $configuredTargetDirectory) {
            [Environment]::SetEnvironmentVariable(
                'CARGO_TARGET_DIR',
                $configuredTargetDirectory,
                'Process'
            )
        }
    }
    $workspaceMemberId = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($id in $metadata.workspace_members) {
        [void] $workspaceMemberId.Add([string] $id)
    }

    foreach ($package in $metadata.packages | Sort-Object -Property name) {
        if (-not $workspaceMemberId.Contains([string] $package.id)) {
            continue
        }

        [pscustomobject]@{
            Name         = [string] $package.name
            Version      = [string] $package.version
            Publishable  = ($null -eq $package.publish) -or ($package.publish.Count -gt 0)
            Package      = $package
        }
    }
}

function Get-PublishableBinaryCrate {
    # Derives the crates this workflow releases: Cargo workspace members publishable to a registry
    # AND owning a `bin` target. In `cargo metadata` the `publish` field is null (any registry), an
    # empty list (never publish), or a non-empty registry list.
    # Returns {Name, Version, Binary, ReleaseTargets} objects sorted by name, where Binary is the
    # package's single binary target and ReleaseTargets is its declared release-target restriction
    # (empty for the usual "all targets" case). A release archive has one binary path, so packages
    # with several binary targets are rejected rather than silently publishing only one. Runs real
    # Cargo metadata; tests point it at a fixture via -ManifestPath.
    [CmdletBinding()]
    param(
        [string] $ManifestPath
    )

    Get-WorkspaceMember -ManifestPath $ManifestPath |
        Where-Object Publishable |
        Where-Object { $_.Package.targets | Where-Object { $_.kind -contains 'bin' } } |
        ForEach-Object {
            $binaryTargets = @(Get-BinaryTarget -Package $_.Package)
            if ($binaryTargets.Count -ne 1) {
                throw (
                    "Publishable binary package '$($_.Name)' declares $($binaryTargets.Count) " +
                    'binary targets; release automation requires exactly one.'
                )
            }
            [pscustomobject]@{
                Name           = $_.Name
                Version        = $_.Version
                Binary         = [string] $binaryTargets[0].name
                ReleaseTargets = @(Get-DeclaredReleaseTarget -Package $_.Package)
            }
        } |
        Sort-Object -Property Name -Unique
}

function Get-BinaryReleaseAsset {
    # Returns the names of the assets already attached to the GitHub release for $Tag, or $null
    # if no such release exists yet. Isolates the real `gh release view` call so the tests can
    # mock it.
    #
    # A non-zero `gh` exit is treated as "no release yet" ONLY when it is the specific "release
    # not found" case; any other failure (auth, network, GitHub API error) is rethrown. Swallowing
    # those would let the caller build an empty/partial matrix, so the binary build is skipped and
    # the workflow looks successful while binaries are still missing.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $Tag,
        [string] $Repository
    )

    # Disable the native-error preference locally so a non-zero exit does not terminate here
    # before we can classify it; we inspect the exit code and output ourselves. 2>&1 merges
    # stderr (where gh prints "release not found") into the captured output.
    $PSNativeCommandUseErrorActionPreference = $false
    $arguments = @('release', 'view', $Tag, '--json', 'assets')
    if ($Repository) { $arguments += @('--repo', $Repository) }
    $output = & gh @arguments 2>&1
    $exitCode = $LASTEXITCODE

    if ($exitCode -ne 0) {
        $text = ($output | Out-String).Trim()
        if ($text -match 'release not found') { return $null }
        throw "gh release view '$Tag' failed (exit $exitCode): $text"
    }

    # An existing-but-empty release returns @() (all target asset pairs missing), distinct from
    # $null ("no release yet"). The guard also keeps member enumeration strict-mode-safe.
    $parsed = ($output | Out-String) | ConvertFrom-Json
    if (-not $parsed.assets) { return , @() }
    , @($parsed.assets.name)
}

function Invoke-ReleasePublish {
    # Publishes changed crates to crates.io via `release-plz release` using the registry-only
    # config, with bounded retries. release-plz is idempotent (it skips already-published
    # versions), so a retry or a whole re-run safely resumes a partially-published release. NOT
    # for local use: it performs real publishes. The native-error preference is disabled locally
    # so a non-zero exit is handled here (turned into a retryable failure) rather than aborting.
    [Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSReviewUnusedParameter', 'ConfigPath',
        Justification = 'Consumed inside the -Action retry closure (release-plz --config), which the rule does not trace into.')]
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $ConfigPath,
        [int] $Attempt = 3,
        [int] $DelaySeconds = 900
    )

    $PSNativeCommandUseErrorActionPreference = $false
    Invoke-WithRetry -Attempt $Attempt -DelaySeconds $DelaySeconds -Action {
        release-plz release --config $ConfigPath
        if ($LASTEXITCODE -ne 0) {
            throw "release-plz release exited with code $LASTEXITCODE"
        }
    }
}

function Get-PublishableCrate {
    # Every Cargo workspace crate publishable to a registry (unlike Get-PublishableBinaryCrate,
    # not filtered to binaries), as {Name, Version} objects sorted by name. Tag reconciliation
    # uses this inventory to complete the registry publication's package requests.
    # Runs real Cargo metadata; tests point it at a fixture workspace via -ManifestPath.
    [CmdletBinding()]
    param(
        [string] $ManifestPath
    )

    Get-WorkspaceMember -ManifestPath $ManifestPath |
        Where-Object Publishable |
        ForEach-Object { [pscustomobject]@{ Name = $_.Name; Version = $_.Version } } |
        Sort-Object -Property Name -Unique
}

function Set-GitHubOutput {
    # Emits a `name=value` step output for the workflow (and echoes it for the run log). No-ops
    # the file append when GITHUB_OUTPUT is unset, so the recipes are runnable locally.
    # Empty values are opt-in because most workflow outputs, including release-asset outputs, are
    # contracts whose absence must not be hidden behind a syntactically present output line.
    [CmdletBinding(SupportsShouldProcess)]
    param(
        [Parameter(Mandatory)][string] $Name,
        [Parameter(Mandatory)][AllowEmptyString()][string] $Value,
        [switch] $AllowEmptyValue
    )

    if ($Value.Length -eq 0 -and -not $AllowEmptyValue) {
        throw "GitHub output '$Name' must not be empty."
    }

    Write-Host "$Name=$Value"
    if ($env:GITHUB_OUTPUT -and $PSCmdlet.ShouldProcess($env:GITHUB_OUTPUT, "append output '$Name'")) {
        Add-Content -Path $env:GITHUB_OUTPUT -Value "$Name=$Value" -Encoding utf8
    }
}

Export-ModuleMember -Function `
    Get-ReleaseTarget, `
    Get-DeclaredReleaseTarget, `
    Get-BinaryTarget, `
    Get-PublishableBinaryCrate, `
    Get-PublishableCrate, `
    Get-BinaryReleaseAsset, `
    Invoke-ReleasePublish, `
    Set-GitHubOutput
