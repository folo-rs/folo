#requires -Version 7

# In-repo replacement for a marketplace toolchain action, run by .github/actions/setup-environment.
# It installs the pinned stable toolchain from rust-toolchain.toml and sets only the Cargo
# environment variables our pipeline actually relies on.
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

function Install-RustToolchain {
    # Installs the pinned stable toolchain via rustup and sets the Cargo environment the composite
    # relies on. Parameters exist purely for testability; in CI both take their real defaults.
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

    # The install writes a lot to disk and pulls from static.rust-lang.org; either can blip
    # transiently (the os error 5 that motivated owning this step), so retry the install alone with
    # a short exponential backoff. RUSTUP_PERMIT_COPY_RENAME mirrors the marketplace action's
    # handling of cross-device installs.
    $env:RUSTUP_PERMIT_COPY_RENAME = '1'
    Invoke-WithRetry -Attempt 4 -DelaySeconds 5 -BackoffMultiplier 2 -MaxDelaySeconds 30 -Action {
        rustup toolchain install $channel --profile minimal --no-self-update
        if ($LASTEXITCODE -ne 0) {
            throw "rustup toolchain install $channel exited with code $LASTEXITCODE"
        }
    }

    # Making the pinned channel the default is convenient but not essential (rust-toolchain.toml
    # already directs cargo to it), so a failure here must not fail environment setup.
    rustup default $channel
    if ($LASTEXITCODE -ne 0) {
        Write-Warning "rustup default $channel exited with code $LASTEXITCODE (continuing)"
    }

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
    Install-RustToolchain
