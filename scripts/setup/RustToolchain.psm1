#requires -Version 7.6

# Prepares the pinned toolchain set for .github/actions/setup-environment and just install-tools.
# CI completes this before Rust cache lookup, so restored and newly installed environments
# present the same compiler inventory. Ref: .github/workflows/implementation.md#shared-environment-cache-identity.
#
# It is a standalone module (imported via `shell: pwsh`), NOT a `just` recipe: the composite installs
# the toolchain BEFORE `just` is available, so `just` cannot be used here.
#
# Owning the step buys two things over a marketplace action: it drops a floating `@master`
# supply-chain dependency, and - the reason it exists - it wraps `rustup toolchain install` in an
# item-level retry, because that install pulls from package mirrors onto runner disks where a
# transient fault (a network blip or a disk I/O error) is not a real failure and must not drop the
# whole job.

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

Import-Module (Join-Path $PSScriptRoot '..' 'utility' 'Constants.psm1') -Force
Import-Module (Join-Path $PSScriptRoot '..' 'utility' 'Retry.psm1') -Force

function Get-PinnedRustChannel {
    # Reads the pinned stable channel from rust-toolchain.toml so the version is never hardcoded -
    # the manifest stays the single source of truth. Parsing in PowerShell keeps it portable across
    # the Linux/macOS/Windows runners (macOS ships BSD grep, which has no -P).
    [CmdletBinding()]
    [OutputType([string])]
    param(
        [string] $ManifestPath = 'rust-toolchain.toml'
    )

    $match = Select-String -Path $ManifestPath -Pattern '^\s*channel\s*=\s*"([^"]+)"' | Select-Object -First 1
    if (-not $match) {
        throw "Could not read the pinned channel from $ManifestPath"
    }

    return $match.Matches[0].Groups[1].Value
}

function Set-CargoEnvDefault {
    # Exports $Name=$Value to $GitHubEnvPath (GITHUB_ENV) for subsequent workflow steps, but only if
    # the variable is not already set in the process environment - so an explicit value configured
    # by the workflow always wins over this default. Writing to GITHUB_ENV
    # affects later steps, not this one, which is why the "already set" check reads the live process
    # environment rather than the file.
    [CmdletBinding(SupportsShouldProcess)]
    param(
        [Parameter(Mandatory)][string] $Name,
        [Parameter(Mandatory)][string] $Value,
        [string] $GitHubEnvPath
    )

    if (Test-Path "env:$Name") {
        Write-Host "$Name already set to '$((Get-Item "env:$Name").Value)'; leaving it unchanged"
        return
    }

    if ([string]::IsNullOrEmpty($GitHubEnvPath)) {
        Write-Warning "GITHUB_ENV is not set; cannot export $Name=$Value"
        return
    }

    if ($PSCmdlet.ShouldProcess($GitHubEnvPath, "append environment '$Name'")) {
        Add-Content -Path $GitHubEnvPath -Value "$Name=$Value" -Encoding utf8
        Write-Host "Exported $Name=$Value via GITHUB_ENV"
    }
}

function Install-RustupToolchain {
    # Installs one toolchain (or the active rust-toolchain.toml toolchain when Channel is omitted)
    # with item-level retry. Components are passed through individually so a large nightly install
    # is retried as one internally consistent rustup operation.
    #
    # The retry is deliberately unconditional (no Test-TransientFailure predicate). The faults it
    # exists to absorb - a runner disk I/O error (the `os error 5` that motivated owning this step)
    # or a mid-download network blip - are not reliably classifiable from rustup's message, and the
    # install is idempotent, so every failure is re-attempted within the bounded backoff; only a
    # genuinely deterministic failure (e.g. a bad channel pin) burns the full window before it
    # surfaces. rustup streams its own diagnostics to the log on each attempt, so throwing just the
    # exit code keeps the failure legible without buffering that live stream - matching how the
    # release-plz wrapper reports.
    [CmdletBinding()]
    param(
        [string] $Channel,
        [ValidateSet('minimal', 'default', 'complete')][string] $InstallProfile,
        [string[]] $Component = @()
    )

    $PSNativeCommandUseErrorActionPreference = $false

    $rustupArguments = @('toolchain', 'install')
    if (-not [string]::IsNullOrWhiteSpace($Channel)) {
        $rustupArguments += $Channel
    }
    if (-not [string]::IsNullOrWhiteSpace($InstallProfile)) {
        $rustupArguments += @('--profile', $InstallProfile)
    }
    foreach ($name in $Component) {
        $rustupArguments += @('--component', $name)
    }
    $rustupArguments += '--no-self-update'

    $toolchainDescription = if ([string]::IsNullOrWhiteSpace($Channel)) {
        'the active toolchain'
    } else {
        $Channel
    }

    $env:RUSTUP_PERMIT_COPY_RENAME = '1'
    Invoke-WithRetry -Attempt 4 -DelaySeconds 5 -BackoffMultiplier 2 -MaxDelaySeconds 30 -Action {
        rustup @rustupArguments
        if ($LASTEXITCODE -ne 0) {
            throw "rustup toolchain install for $toolchainDescription exited with code $LASTEXITCODE"
        }
    }
}

function Install-RustToolchainSet {
    # CI cannot use Just before cache restoration. Keep its toolchain set and the local recipe's
    # identical by reading the same pins here, without exporting dotenv inputs into the cache's
    # environment. Rustup reconciles requested components even after a partial cache restore.
    [CmdletBinding()]
    param(
        [string] $ConstantsPath = (Join-Path $PSScriptRoot '..' '..' 'constants.env')
    )

    $constants = Read-DotEnvFile -Path $ConstantsPath
    $msrv = Get-RequiredConstant -Values $constants -Name 'RUST_MSRV'
    $nightly = Get-RequiredConstant -Values $constants -Name 'RUST_NIGHTLY'
    $externalTypes = Get-RequiredConstant -Values $constants -Name 'RUST_NIGHTLY_EXTERNAL_TYPES'

    # Let rustup resolve the stable channel and its components from rust-toolchain.toml.
    Install-RustupToolchain
    Install-RustupToolchain -Channel $msrv
    Install-RustupToolchain -Channel $nightly `
        -Component @('miri', 'rustfmt', 'rust-src', 'llvm-tools-preview')

    # This schema-paired nightly only drives rustdoc. Ref: constants.env,
    # RUST_NIGHTLY_EXTERNAL_TYPES; the general nightly owns the additional analysis components.
    Install-RustupToolchain -Channel $externalTypes -InstallProfile minimal
}

function Install-RustToolchain {
    # Selects CI's stable default and prepares every toolchain before the shared cache lookup.
    # Local installation uses Install-RustToolchainSet without changing rustup's default.
    [CmdletBinding()]
    param(
        [string] $ManifestPath = 'rust-toolchain.toml',
        [string] $GitHubEnvPath = $env:GITHUB_ENV
    )

    # `rustup default` is intentionally best-effort, so handle native exit codes explicitly rather
    # than letting the composite step's $PSNativeCommandUseErrorActionPreference auto-throw on them.
    $PSNativeCommandUseErrorActionPreference = $false

    $channel = Get-PinnedRustChannel -ManifestPath $ManifestPath
    Write-Host "Pinned Rust channel from ${ManifestPath}: $channel"

    Install-RustupToolchain -Channel $channel -InstallProfile minimal

    # Making the pinned channel the default is convenient but not essential (rust-toolchain.toml
    # already directs cargo to it), so a failure here must not fail environment setup.
    rustup default $channel
    if ($LASTEXITCODE -ne 0) {
        Write-Warning "rustup default $channel exited with code $LASTEXITCODE (continuing)"
    }

    Install-RustToolchainSet

    Set-CargoEnvDefault -Name 'CARGO_INCREMENTAL' -Value '0' -GitHubEnvPath $GitHubEnvPath
    Set-CargoEnvDefault -Name 'CARGO_TERM_COLOR' -Value 'always' -GitHubEnvPath $GitHubEnvPath

    # Surface the resolved compiler in the log, exactly like the marketplace action did.
    rustc "+$channel" --version --verbose
    if ($LASTEXITCODE -ne 0) {
        throw "rustc +$channel --version exited with code $LASTEXITCODE"
    }
}

Export-ModuleMember -Function `
    Get-PinnedRustChannel, `
    Set-CargoEnvDefault, `
    Install-RustupToolchain, `
    Install-RustToolchainSet, `
    Install-RustToolchain
