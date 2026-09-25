#requires -Version 7.6

# Shared binary-first Cargo tool installation for local setup, Just recipes and CI.
# PowerShell owns this boundary because it must bootstrap Just before Rust helpers can run.
# Publisher archives are preferred; Git-pinned and known source-only tools stay in the recipe.
# Ref: docs/build-and-tooling.md#development-tool-installation.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

Import-Module (Join-Path $PSScriptRoot '..' 'utility' 'Constants.psm1') -Force
Import-Module (Join-Path $PSScriptRoot '..' 'utility' 'Retry.psm1') -Force

# SHA-256 digests of the official cargo-binstall release assets. Update these together with
# CARGO_BINSTALL_VERSION in constants.env, using the release API's asset digests.
# Refresh the pinned upstream references here when updating that version.
# Archive names/layout follow the upstream manual installation instructions:
# https://github.com/cargo-bins/cargo-binstall/blob/v1.23.0/README.md#manually.
# Digest source: https://api.github.com/repos/cargo-bins/cargo-binstall/releases/tags/v1.23.0.
# Linux uses the static musl build to avoid depending on the host's glibc version.
$script:BinstallBuilds = @{
    'windows-X64'   = @{ Target = 'x86_64-pc-windows-msvc'; Format = 'zip'; Sha256 = 'f4641479477aca40387e88297e3813fab8e44a8d21f25faa44e0ff33e2bc1726' }
    'windows-Arm64' = @{ Target = 'aarch64-pc-windows-msvc'; Format = 'zip'; Sha256 = 'a3996cbd0c7fa2be599bd52eaf70ccd8c0dce4cce8d01345d486f47b0e0ee0f9' }
    'linux-X64'     = @{ Target = 'x86_64-unknown-linux-musl'; Format = 'tgz'; Sha256 = '64bf954c68bb558431deeabecaec7687edd5541c2189ee263bb8bc18bc4fdf55' }
    'linux-Arm64'   = @{ Target = 'aarch64-unknown-linux-musl'; Format = 'tgz'; Sha256 = 'ba9b7bf426c7b7375825cd3fa367c3f8a632ca7c6c546fdcb738114b167f4103' }
    'mac-X64'       = @{ Target = 'x86_64-apple-darwin'; Format = 'zip'; Sha256 = '5b4d6cd99651318e17a26ed9bab7505de0f496d88793a7735a0fc6e2079efe83' }
    'mac-Arm64'     = @{ Target = 'aarch64-apple-darwin'; Format = 'zip'; Sha256 = '0f679c0bc992c6b84fdcbb0f65492588447d26596ef387cb6cb1ba41ad8ceb33' }
}

function Get-CargoInstallRoot {
    # Select one root for bootstrap, binary installs and source installs, including metadata.
    # Binstall documents the environment precedence in its --root option:
    # https://github.com/cargo-bins/cargo-binstall/blob/v1.23.0/HELP.md#cargo-binstall.
    # Cargo also reads install.root, but passing our resolved --root deliberately overrides it
    # so every installation path agrees, without implementing Cargo's configuration discovery.
    # https://doc.rust-lang.org/cargo/commands/cargo-install.html#description.
    [CmdletBinding()]
    [OutputType([string])]
    param()

    $root = if ($env:CARGO_INSTALL_ROOT) { $env:CARGO_INSTALL_ROOT }
        elseif ($env:CARGO_HOME) { $env:CARGO_HOME }
        else { Join-Path $HOME '.cargo' }
    return $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($root)
}

function Get-BootstrapToolVersion {
    # Read pins before Just exists to load its dotenv file. Bootstrap and command-runner setup
    # require exact release identities, not requirements that resolve differently across runs.
    [CmdletBinding()]
    [OutputType([string])]
    param(
        [Parameter(Mandatory)][ValidateSet('CARGO_BINSTALL', 'JUST')][string] $Name
    )

    $values = Read-DotEnvFile -Path (Join-Path $PSScriptRoot '..' '..' 'constants.env')
    $version = Get-RequiredConstant -Values $values -Name "${Name}_VERSION"
    if ($version -notmatch '^\d+\.\d+\.\d+$') {
        throw "The $Name bootstrap version must be an exact release version, got '$version'."
    }
    return $version
}

function Get-BinstallBuild {
    # Match the executing PowerShell process, including x64 processes on ARM64 hosts.
    # Only explicitly supported targets have reviewed archive digests; never guess a fallback.
    [CmdletBinding()]
    [OutputType([hashtable])]
    param(
        [string] $Platform = $(if ($IsWindows) { 'windows' } elseif ($IsLinux) { 'linux' } else { 'mac' }),
        [string] $Architecture = [System.Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture
    )

    $build = $script:BinstallBuilds["$Platform-$Architecture"]
    if ($null -eq $build) {
        throw "cargo-binstall bootstrap does not support '$Platform-$Architecture'."
    }
    return $build
}

function Get-BinstallVersion {
    # Probe the selected bootstrap executable instead of trusting its filename or install metadata.
    # The pinned binstall's -V emits a bare release triplet. An unreadable/malformed executable
    # is an error, not a cache miss to hide by replacing it.
    [CmdletBinding()]
    [OutputType([string])]
    param([Parameter(Mandatory)][string] $Path)

    # Preserve both the path and captured output in our diagnostic for a native-command failure.
    $PSNativeCommandUseErrorActionPreference = $false
    $output = & $Path -V
    if ($LASTEXITCODE -ne 0 -or "$output" -notmatch '^\d+\.\d+\.\d+$') {
        throw "Could not determine cargo-binstall's version at '$Path': $output"
    }
    return "$output"
}

function Install-CargoBinstall {
    # Establish the verified installer that both the pre-Just entry point and recipes use.
    # Reuse a matching executable; otherwise download and verify in private staging before
    # replacing the selected root's copy. Return the installed executable's path without changing PATH.
    # Ref: docs/build-and-tooling.md#development-tool-installation.
    [CmdletBinding()]
    [OutputType([string])]
    param([string] $Root = (Get-CargoInstallRoot))

    $version = Get-BootstrapToolVersion -Name CARGO_BINSTALL
    $build = Get-BinstallBuild
    $binaryName = if ($IsWindows) { 'cargo-binstall.exe' } else { 'cargo-binstall' }
    $destination = Join-Path $Root 'bin'
    $installed = Join-Path $destination $binaryName
    # Always use the selected installation root, not a possibly different tool earlier on PATH.
    if ((Test-Path -LiteralPath $installed) -and (Get-BinstallVersion -Path $installed) -eq $version) {
        Write-Verbose "cargo-binstall $version is already present at '$installed'."
        return $installed
    }

    $asset = "cargo-binstall-$($build.Target).$($build.Format)"
    $url = "https://github.com/cargo-bins/cargo-binstall/releases/download/v$version/$asset"
    Write-Host "Bootstrapping cargo-binstall $version from its official $($build.Target) archive."
    $work = New-Item -ItemType Directory -Path (
        Join-Path ([IO.Path]::GetTempPath()) ("folo-binstall-" + [guid]::NewGuid())
    )
    try {
        $archive = Join-Path $work.FullName $asset
        # A short, capped exponential retry budget tolerates transient release-host failures.
        # Retry only the download/verification, not extraction or execution of a bootstrap.
        Invoke-WithRetry -Attempt 4 -DelaySeconds 3 -BackoffMultiplier 2 -MaxDelaySeconds 30 -Action {
            Invoke-WebRequest -Uri $url -OutFile $archive
            $actual = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($actual -ne $build.Sha256) {
                throw "cargo-binstall archive SHA-256 mismatch: expected $($build.Sha256), got $actual."
            }
        }
        if ($build.Format -eq 'zip') {
            Expand-Archive -LiteralPath $archive -DestinationPath $work.FullName
        } else {
            tar -xf $archive -C $work.FullName
            if ($LASTEXITCODE -ne 0) { throw "Could not extract '$archive'." }
        }
        $binary = Join-Path $work.FullName $binaryName
        if (-not $IsWindows) {
            # ZIP extraction on macOS does not preserve the executable mode.
            chmod +x $binary
            if ($LASTEXITCODE -ne 0) { throw "Could not mark '$binary' executable." }
        }
        if ((Get-BinstallVersion -Path $binary) -ne $version) {
            throw "The verified cargo-binstall archive does not contain version $version."
        }
        $null = New-Item -ItemType Directory -Path $destination -Force
        Copy-Item -LiteralPath $binary -Destination $installed -Force
        if ((Get-BinstallVersion -Path $installed) -ne $version) {
            throw "cargo-binstall $version was not installed at '$installed'."
        }
        return $installed
    } finally {
        Remove-Item -LiteralPath $work.FullName -Recurse -Force
    }
}

function Invoke-CargoBinstall {
    # Keep native exit-code handling shared and send installer progress to the host, not the
    # success pipeline used by helpers to return paths and versions.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $Executable,
        [Parameter(Mandatory)][string[]] $ArgumentList
    )

    $PSNativeCommandUseErrorActionPreference = $false
    & $Executable @ArgumentList | Out-Host
    if ($LASTEXITCODE -ne 0) {
        throw "cargo-binstall exited with code $LASTEXITCODE."
    }
}

function Install-CargoTool {
    # Reconcile the recipe's exact pins through one local/CI installation policy. Leave matching
    # installations tracked and reusable rather than forcing replacement on every setup.
    # PackageUrl accommodates a publisher's nonstandard release layout for one package only.
    # CLI/root/tracking/fallback semantics:
    # https://github.com/cargo-bins/cargo-binstall/blob/v1.23.0/HELP.md#cargo-binstall.
    # URL templates and strategy-override precedence:
    # https://github.com/cargo-bins/cargo-binstall/blob/v1.23.0/SUPPORT.md.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string[]] $Package,
        [string] $Root = (Get-CargoInstallRoot),
        [string] $PackageUrl
    )

    # Exact pins permit both upgrades and downgrades without forcing matching installs.
    foreach ($specification in $Package) {
        if ($specification -notmatch '^[a-zA-Z0-9_-]+@=?\d+\.\d+\.\d+$') {
            throw "Cargo tools require exact crate@version pins, got '$specification'."
        }
        $overrides = @()
        if (-not [string]::IsNullOrWhiteSpace($PackageUrl)) {
            if ($Package.Count -ne 1) {
                throw 'A publisher URL override requires exactly one package.'
            }
            $overrides = @('--pkg-url', $PackageUrl)
        }
    }
    $executable = Install-CargoBinstall -Root $Root
    Write-Host "Installing pinned Cargo tools from publisher binaries, with locked source fallback: $($Package -join ', ')"
    # Keep publisher-disabled strategies effective. Explicit --strategies would override them.
    # Disable credential discovery: CI supplies its job token; local public downloads need no login.
    # Ref: docs/build-and-tooling.md#development-tool-installation.
    Invoke-CargoBinstall -Executable $executable -ArgumentList (
        @('--no-confirm', '--locked', '--disable-strategies', 'quick-install',
            '--disable-telemetry', '--no-discover-github-token', '--root', $Root) + $overrides + $Package
    )
}

Export-ModuleMember -Function Get-CargoInstallRoot, Get-BootstrapToolVersion, Install-CargoBinstall, Install-CargoTool
