# Runs the native Bicep compiler for maintained templates and parameter files without deployment.
# Called by just validate-bicep; Bicep owns syntax, type/API catalog and linter decisions.
# Ref: docs/build-and-tooling.md, "Bicep validation".
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

function Get-FoloBicepPath {
    # Share Azure CLI's conventional compiler location, but install for this process's platform.
    [CmdletBinding()]
    [OutputType([string])]
    param()

    $configuration = if ($env:AZURE_CONFIG_DIR) { $env:AZURE_CONFIG_DIR } else { Join-Path $HOME '.azure' }
    Join-Path $configuration 'bin' $(if ($IsWindows) { 'bicep.exe' } else { 'bicep' })
}

function Invoke-BicepProcess {
    # Captures native diagnostics separately from generated ARM output without a shell.
    [CmdletBinding()]
    [OutputType([pscustomobject])]
    param(
        [Parameter(Mandatory)][string] $Executable,
        [Parameter(Mandatory)][string[]] $Arguments,
        [hashtable] $Environment = @{}
    )

    $start = [Diagnostics.ProcessStartInfo]::new($Executable)
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    foreach ($argument in $Arguments) { $start.ArgumentList.Add($argument) }
    foreach ($name in $Environment.Keys) { $start.Environment[$name] = $Environment[$name] }
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $start
    try {
        $null = $process.Start()
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        $process.WaitForExit()
        [pscustomobject]@{
            ExitCode = $process.ExitCode
            Stdout = $stdout.GetAwaiter().GetResult()
            Stderr = $stderr.GetAwaiter().GetResult()
        }
    } finally {
        $process.Dispose()
    }
}

function Get-BicepDiagnosticFailure {
    # Compiler warnings and linter warnings share SARIF severity and both fail validation.
    [CmdletBinding()]
    [OutputType([string[]])]
    param([Parameter(Mandatory)][AllowEmptyString()][string] $Diagnostics)

    $sarif = ConvertFrom-Json -InputObject $Diagnostics -AsHashtable
    if ($sarif -isnot [hashtable] -or $sarif.version -ne '2.1.0' -or $sarif.runs -isnot [array]) {
        throw 'Bicep did not emit a supported SARIF diagnostic document.'
    }
    foreach ($run in $sarif.runs) {
        if ($run -isnot [hashtable] -or $run.results -isnot [array]) {
            throw 'Bicep emitted an invalid SARIF run.'
        }
        foreach ($result in $run.results) {
            if ($result -isnot [hashtable]) { throw 'Bicep emitted an invalid diagnostic.' }
            # SARIF defines an omitted result level as warning, not successful absence.
            $level = if ($result.ContainsKey('level')) { $result.level } else { 'warning' }
            if ($level -notin @('none', 'note', 'warning', 'error')) {
                throw "Bicep emitted an unknown diagnostic severity '$level'."
            }
            if ($level -in @('warning', 'error')) {
                "$($result.ruleId): $($result.message.text)"
            }
        }
    }
}

function Install-FoloBicep {
    # Install the host-native compiler from a pinned, checksum-verified official release.
    # Calling az here could select a Windows CLI through WSL interop and install the wrong binary.
    [CmdletBinding()]
    param([Parameter(Mandatory)][string] $Version)

    $executable = Get-FoloBicepPath
    if (Test-Path -LiteralPath $executable -PathType Leaf) {
        $current = Invoke-BicepProcess -Executable $executable -Arguments @('--version')
        if ($current.ExitCode -eq 0 -and $current.Stdout -match "^Bicep CLI version $([regex]::Escape($Version))(?:\s|$)") {
            Write-Verbose "Pinned Bicep compiler is already installed at '$executable'."
            return
        }
    }
    $os = if ($IsWindows) { 'win' } elseif ($IsMacOS) { 'osx' } else { 'linux' }
    $arch = switch ([Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture) {
        ([Runtime.InteropServices.Architecture]::X64) { 'x64' }
        ([Runtime.InteropServices.Architecture]::Arm64) { 'arm64' }
        default { throw 'The pinned Bicep installer supports x64 and ARM64.' }
    }
    $asset = "bicep-$os-$arch" + $(if ($IsWindows) { '.exe' } else { '' })
    # Published asset digests from https://github.com/Azure/bicep/releases/tag/v0.44.1.
    # The catalog is keyed by the configured pin; a pin change requires reviewed digests.
    $digests = @{
        '0.44.1' = @{
            'bicep-linux-arm64' = 'a9c73b96975a17e49b5dd66725c8dc2451501c77b194156d938719ee4990318b'
            'bicep-linux-x64' = 'e17dc9a9888184886bb0c0051a3230b83b19f342749999f707bc571c3dfd2f45'
            'bicep-osx-arm64' = 'd96a185cd7ce6a685a9d43130ff2298597603d7be9f782c7bef5171ed6884795'
            'bicep-osx-x64' = '9d3c8f412a82670b2d89126eda5210ce1290a5e3790f373094ea98c813cd90a9'
            'bicep-win-arm64.exe' = '400ce5b9451b386d9026beca3850c23e208489c3cf1ec0c02508dd7d2206563e'
            'bicep-win-x64.exe' = '5153ab5898e70f1f822e0b844ae902a5acc5692f5a6303814af7b202e1072160'
        }
    }
    if (-not $digests.ContainsKey($Version)) {
        throw "No reviewed Bicep checksums exist for pin '$Version'."
    }
    $directory = Split-Path -Parent $executable
    New-Item -ItemType Directory -Path $directory -Force | Out-Null
    $download = Join-Path $directory (".bicep-download-$([guid]::NewGuid().ToString('N'))")
    try {
        Invoke-WebRequest -Uri "https://github.com/Azure/bicep/releases/download/v$Version/$asset" `
            -OutFile $download -MaximumRetryCount 3 -RetryIntervalSec 2
        $actual = (Get-FileHash -LiteralPath $download -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actual -cne $digests[$Version][$asset]) {
            throw "Bicep checksum mismatch for '$asset'."
        }
        if (-not $IsWindows) { chmod +x $download }
        Move-Item -LiteralPath $download -Destination $executable -Force
    } finally {
        if (Test-Path -LiteralPath $download) { Remove-Item -LiteralPath $download -Force }
    }
    $installed = Invoke-BicepProcess -Executable $executable -Arguments @('--version')
    if ($installed.ExitCode -ne 0 -or $installed.Stdout -notmatch "^Bicep CLI version $([regex]::Escape($Version))(?:\s|$)") {
        throw "The installed Bicep compiler does not match the repository pin '$Version'."
    }
}

function Invoke-BicepValidation {
    # Compile every maintained template and parameter file to diagnostics/build output, never Azure.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $RepositoryRoot,
        [Parameter(Mandatory)][string] $Version,
        [Parameter(Mandatory)][string] $DiagnosticsDirectory
    )

    $executable = Get-FoloBicepPath
    if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
        throw 'The repository Bicep compiler is missing. Run just install-bicep.'
    }
    $current = Invoke-BicepProcess -Executable $executable -Arguments @('--version')
    if ($current.ExitCode -ne 0 -or $current.Stdout -notmatch "^Bicep CLI version $([regex]::Escape($Version))(?:\s|$)") {
        throw "Bicep compiler does not match '$Version'. Run just install-bicep."
    }
    $roots = @(
        (Join-Path $RepositoryRoot 'infra'),
        (Join-Path $RepositoryRoot 'packages' 'cargo-bench-history' 'src' 'azure_bundle')
    )
    $files = @(
        Get-ChildItem -LiteralPath $roots -Recurse -File |
            Where-Object { $_.Extension -cin @('.bicep', '.bicepparam') } |
            Sort-Object FullName
    )
    if ($files.Count -eq 0) { throw 'No maintained Bicep input files were found.' }
    $failures = [Collections.Generic.List[string]]::new()
    foreach ($file in $files) {
        $relative = [IO.Path]::GetRelativePath($RepositoryRoot, $file.FullName)
        $output = Join-Path $DiagnosticsDirectory "$relative.json"
        New-Item -ItemType Directory -Path (Split-Path -Parent $output) -Force | Out-Null
        $command = if ($file.Extension -ceq '.bicepparam') { 'build-params' } else { 'build' }
        Write-Host "Checking Bicep input $relative"
        $result = Invoke-BicepProcess -Executable $executable -Arguments @(
            $command, $file.FullName, '--no-restore', '--diagnostics-format', 'sarif', '--outfile', $output
        ) -Environment @{
            # Parameter-file compilation uses a valid non-secret placeholder, not operator state.
            # These child-process values never reach Azure because the gate only compiles.
            AZURE_STORAGE_ACCOUNT_NAME = 'bicepvalidation'
        }
        Set-Content -LiteralPath "$output.sarif" -Value $result.Stderr -Encoding utf8
        Set-Content -LiteralPath "$output.stdout" -Value $result.Stdout -Encoding utf8
        $diagnostics = @(Get-BicepDiagnosticFailure -Diagnostics $result.Stderr)
        if ($result.ExitCode -ne 0 -or $diagnostics.Count -gt 0) {
            $failures.Add("$relative (exit $($result.ExitCode)): $($diagnostics -join '; ')")
        }
    }
    if ($failures.Count -gt 0) {
        throw "Bicep validation failed: $($failures -join [Environment]::NewLine)"
    }
    Write-Host "Bicep syntax, type/API catalog and lint validation passed. Diagnostics: $DiagnosticsDirectory"
}

Export-ModuleMember -Function Get-FoloBicepPath, Install-FoloBicep, Invoke-BicepValidation
