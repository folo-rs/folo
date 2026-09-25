#requires -Version 7

# Native ZIP prerequisites for release-binaries and its no-upload integration smoke. Called by
# just install-tools on developers' machines and every setup-environment job. PowerShell owns
# this bootstrap boundary because Rust tooling is itself installed by the enclosing recipe.
# Ref: docs/build-and-tooling.md#release-archive-tools.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

Import-Module (Join-Path $PSScriptRoot '..' 'utility' 'Retry.psm1') -Force

# Official ip7z/7zip release asset digest. The extra package contains standalone 7za.exe;
# Windows ARM64 uses its x64 executable under emulation, like the ShellCheck installer.
$script:SevenZipVersion = '26.03'
$script:SevenZipAsset = '7z2603-extra.7z'
$script:SevenZipHash = '191894e6acb3647ffb69ce630479ff318523b2e2b9890aa7f05c1127c2e59b8f'

function Get-ArchivePlatform {
    [CmdletBinding()]
    [OutputType([string])]
    param()

    if ($IsWindows) { return 'windows' }
    if ($IsLinux) { return 'linux' }
    if ($IsMacOS) { return 'macos' }
    throw 'Release archive setup does not support this platform.'
}

function Invoke-ArchivePackageInstall {
    [CmdletBinding()]
    param([Parameter(Mandatory)][ValidateSet('linux', 'macos')][string] $Platform)

    if ($Platform -eq 'macos') {
        $null = Get-Command brew -ErrorAction Stop
        brew install zip unzip | Out-Host
    } else {
        $null = Get-Command apt-get -ErrorAction Stop
        $root = (id -u) -eq '0'
        if (-not $root) {
            $null = Get-Command sudo -ErrorAction Stop
        }
        $command = if ($root) { 'apt-get' } else { 'sudo' }
        $arguments = if ($root) { @() } else { @('apt-get') }
        & $command @arguments update -qq | Out-Host
        & $command @arguments install -y zip unzip | Out-Host
    }
}

function Install-StandaloneSevenZip {
    [CmdletBinding()]
    param([Parameter(Mandatory)][string] $Destination)

    $installed = Join-Path $Destination '7za.exe'
    if ((Test-Path -LiteralPath $installed) -and (Test-StandaloneSevenZip -Path $installed)) {
        Write-Host "Standalone 7-Zip $script:SevenZipVersion is available."
        return
    }
    # Windows' libarchive supports 7z. Select it explicitly: Git's GNU tar can precede it on
    # PATH and interprets a Windows drive letter in the archive path as a remote hostname.
    $tar = Get-Command (Join-Path $env:SystemRoot 'System32' 'tar.exe') -CommandType Application -ErrorAction Stop
    $work = Join-Path ([IO.Path]::GetTempPath()) "release-archive-tools-$([guid]::NewGuid().ToString('N'))"
    $null = New-Item -ItemType Directory -Path $work
    try {
        $archive = Join-Path $work $script:SevenZipAsset
        $url = "https://github.com/ip7z/7zip/releases/download/$script:SevenZipVersion/$script:SevenZipAsset"
        Invoke-WithRetry -Attempt 4 -DelaySeconds 3 -BackoffMultiplier 2 -MaxDelaySeconds 30 -Action {
            Invoke-WebRequest -Uri $url -OutFile $archive
            if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -cne $script:SevenZipHash) {
                throw 'Standalone 7-Zip archive checksum does not match the pinned official asset.'
            }
        }
        & $tar.Source -xf $archive -C $work
        $null = New-Item -ItemType Directory -Path $Destination -Force
        Copy-Item -LiteralPath (Join-Path $work 'x64' '7za.exe') -Destination (Join-Path $Destination '7za.exe') -Force
        Copy-Item -LiteralPath (Join-Path $work 'License.txt') -Destination (Join-Path $Destination 'release-7zip-license.txt') -Force
    } finally {
        Remove-Item -LiteralPath $work -Recurse -Force
    }
}

function Test-StandaloneSevenZip {
    # A cached executable is only usable when the native version probe succeeds.
    # Nonzero exits and loader failures need replacement, not another failed setup attempt.
    [CmdletBinding()]
    [OutputType([bool])]
    param([Parameter(Mandatory)][string] $Path)

    $previousPreference = $PSNativeCommandUseErrorActionPreference
    try {
        $PSNativeCommandUseErrorActionPreference = $false
        $output = & $Path i 2>&1
        $exitCode = $LASTEXITCODE
        if ($exitCode -ne 0) {
            Write-Verbose "Replacing cached archive tool because its version probe exited $exitCode." -Verbose
            return $false
        }
        return [bool] ($output -match "7-Zip.* $([regex]::Escape($script:SevenZipVersion)) ")
    } catch [System.Management.Automation.ApplicationFailedException] {
        Write-Verbose "Replacing cached archive tool because it could not start: $_" -Verbose
        return $false
    } finally {
        $PSNativeCommandUseErrorActionPreference = $previousPreference
    }
}

function Install-ReleaseArchiveTool {
    [CmdletBinding()]
    param([string] $Destination = (Join-Path $(if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $HOME '.cargo' }) 'bin'))

    $platform = Get-ArchivePlatform
    if ($platform -eq 'windows') {
        Install-StandaloneSevenZip -Destination $Destination
        # Make the managed executable authoritative in this setup process and later Actions steps.
        # The Cargo bin directory is the repository's normal local tool PATH prerequisite.
        $env:PATH = $Destination + [IO.Path]::PathSeparator + $env:PATH
        if ($env:GITHUB_PATH) {
            Add-Content -LiteralPath $env:GITHUB_PATH -Value $Destination -Encoding utf8NoBOM
        }
        $tool = Get-Command 7za -CommandType Application -ErrorAction Stop
        & $tool.Source i | Select-Object -First 3 | Out-Host
        return
    }
    $missing = @(@('zip', 'unzip') | Where-Object {
            -not (Get-Command $_ -CommandType Application -ErrorAction SilentlyContinue)
        })
    if ($missing.Count -gt 0) {
        Write-Host "Installing missing release archive tools: $($missing -join ', ')."
        Invoke-ArchivePackageInstall -Platform $platform
    }
    foreach ($tool in @('zip', 'unzip')) {
        $application = Get-Command $tool -CommandType Application -ErrorAction Stop
        & $application.Source -v | Select-Object -First 2 | Out-Host
    }
}

Export-ModuleMember -Function Install-ReleaseArchiveTool
