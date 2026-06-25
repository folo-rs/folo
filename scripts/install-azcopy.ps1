#requires -Version 7

<#
.SYNOPSIS
    Installs the azcopy CLI used by the cargo-bench-history Azure stress harness.

.DESCRIPTION
    azcopy performs the bulk blob upload in `just bench-history-stress-azure`. It is
    not a cargo crate, and Microsoft ships it as a standalone, portable binary with no
    installer and no canonical install location of its own, so this script downloads
    the official static build for the current operating system and drops it on PATH.

    The default destination is the cargo bin directory. That is a deliberate, if
    pragmatic, choice: azcopy has no standard home to prefer instead, and the cargo bin
    directory is the one location this repository's developer environment guarantees is
    already on PATH — every other tool `just install-tools` fetches lands there too.
    Override -Destination to install elsewhere, but that location must be on PATH for
    `just bench-history-stress-azure` to find azcopy.

    Idempotent: if azcopy already resolves on PATH the script does nothing (pass -Force
    to reinstall anyway).

.PARAMETER Destination
    Directory to install the azcopy binary into. Defaults to the cargo bin directory
    (`~/.cargo/bin`), which is already on PATH in this dev environment.

.PARAMETER Force
    Reinstall even if azcopy is already present on PATH.

.EXAMPLE
    ./install-azcopy.ps1
#>
[CmdletBinding()]
param(
    [string] $Destination = (Join-Path $HOME '.cargo/bin'),

    [switch] $Force
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

function Get-AzCopyDownload {
    # Maps the current OS to the official azcopy download: the redirect URL, the
    # archive format it serves, and the binary name inside it. `#requires -Version 7`
    # above guarantees the $Is* automatic variables exist, so the final branch is
    # simply "not macOS and not Linux", i.e. Windows.
    Write-Verbose 'Selecting the azcopy download for the current operating system.'
    if ($IsMacOS) {
        return [pscustomobject]@{ Url = 'https://aka.ms/downloadazcopy-v10-mac'; Archive = 'zip'; BinaryName = 'azcopy' }
    } elseif ($IsLinux) {
        return [pscustomobject]@{ Url = 'https://aka.ms/downloadazcopy-v10-linux'; Archive = 'tar.gz'; BinaryName = 'azcopy' }
    } else {
        return [pscustomobject]@{ Url = 'https://aka.ms/downloadazcopy-v10-windows'; Archive = 'zip'; BinaryName = 'azcopy.exe' }
    }
}

function Expand-AzCopyArchive {
    param(
        [Parameter(Mandatory)] [string] $ArchivePath,
        [Parameter(Mandatory)] [string] $ArchiveKind,
        [Parameter(Mandatory)] [string] $DestinationDir
    )
    Write-Verbose "Extracting '$ArchivePath' ($ArchiveKind) into '$DestinationDir'."
    if ($ArchiveKind -eq 'tar.gz') {
        tar -xf $ArchivePath -C $DestinationDir
    } else {
        Expand-Archive -Path $ArchivePath -DestinationPath $DestinationDir -Force
    }
}

if (-not $Force -and (Get-Command azcopy -ErrorAction SilentlyContinue)) {
    Write-Host 'azcopy already installed; skipping.'
    return
}

$download = Get-AzCopyDownload
Write-Host "Installing azcopy (used by 'just bench-history-stress-azure')..."
Write-Verbose "Resolved download URL '$($download.Url)' and expected binary name '$($download.BinaryName)'."

Write-Verbose "Ensuring the destination directory '$Destination' exists."
New-Item -ItemType Directory -Force -Path $Destination | Out-Null

# Stage the download in a throwaway temp directory so a failed extract leaves nothing
# behind; the finally block removes it regardless of outcome.
$work = New-Item -ItemType Directory -Force -Path (Join-Path ([System.IO.Path]::GetTempPath()) ("azcopy-" + [guid]::NewGuid()))
Write-Verbose "Staging the download in temp directory '$($work.FullName)'."
try {
    $archive = Join-Path $work ("azcopy." + $download.Archive)
    Write-Verbose "Downloading azcopy archive to '$archive'."
    Invoke-WebRequest -Uri $download.Url -OutFile $archive

    Expand-AzCopyArchive -ArchivePath $archive -ArchiveKind $download.Archive -DestinationDir $work

    Write-Verbose "Locating '$($download.BinaryName)' inside the extracted archive."
    $bin = Get-ChildItem -Path $work -Recurse -Filter $download.BinaryName | Select-Object -First 1
    if ($null -eq $bin) {
        throw "azcopy binary ('$($download.BinaryName)') was not found inside the downloaded archive."
    }

    Write-Verbose "Copying '$($bin.FullName)' to '$Destination'."
    Copy-Item -Path $bin.FullName -Destination $Destination -Force
    Write-Host "Installed azcopy to $Destination."
} finally {
    Write-Verbose "Cleaning up temp directory '$($work.FullName)'."
    Remove-Item -Path $work -Recurse -Force -ErrorAction SilentlyContinue
}
