#requires -Version 7.6

# Controller compilation and the private binary protocol for release planning and platform jobs.
# Rust owns grouping, asset completeness and execution. Both bootstrap controllers are compiled
# from the invocation checkout, never from a tagged binary-source or candidate worktree.
# Ref: packages/release-binaries/docs/implementation.md.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

Import-Module (Join-Path $PSScriptRoot '..' 'build' 'CargoExecutable.psm1') -Force

function Get-ReleaseControllerExecutable {
    [CmdletBinding()]
    [OutputType([string])]
    param([Parameter(Mandatory)][ValidateSet('release-binaries', 'release-target-check')][string] $Package)

    # Preserve distinct spellings on hosts whose process environment is case-sensitive.
    $tokens = [Collections.Generic.Dictionary[string, object]]::new([StringComparer]::Ordinal)
    $errors = [Collections.Generic.List[Exception]]::new()
    $entered = $false
    $executable = $null
    $names = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($name in @('GH_TOKEN', 'GITHUB_TOKEN', 'GIT_TOKEN', 'INPUT_TOKEN', 'DEFAULT_GITHUB_TOKEN')) {
        $null = $names.Add($name)
    }
    foreach ($name in @('CARGO_REGISTRY_TOKEN', 'ACTIONS_ID_TOKEN_REQUEST_URL', 'ACTIONS_ID_TOKEN_REQUEST_TOKEN')) {
        $null = $names.Add($name)
    }
    foreach ($name in [Environment]::GetEnvironmentVariables('Process').Keys) {
        # Match the Cargo registry-token family, not ordinary registry indices or build settings.
        if ($name -ilike 'CARGO_REGISTRIES_*_TOKEN') { $null = $names.Add($name) }
    }
    try {
        Push-Location (Join-Path $PSScriptRoot '..' '..')
        $entered = $true
        foreach ($name in $names) {
            $tokens[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
            # PowerShell converts an ordinary null string argument to an empty value.
            # Explicit NullString removes the variable rather than forwarding an empty credential.
            [Environment]::SetEnvironmentVariable($name, [NullString]::Value, 'Process')
        }
        $messages = @(cargo build --package $Package --locked --message-format=json-render-diagnostics)
        $executable = Resolve-CargoExecutable -CargoMessage $messages -TargetName $Package
    } catch {
        $errors.Add($_.Exception)
    } finally {
        foreach ($name in $tokens.Keys) {
            try {
                if ($null -eq $tokens[$name]) {
                    [Environment]::SetEnvironmentVariable($name, [NullString]::Value, 'Process')
                } else {
                    [Environment]::SetEnvironmentVariable($name, $tokens[$name], 'Process')
                }
            } catch { $errors.Add($_.Exception) }
        }
        if ($entered) {
            try { Pop-Location } catch { $errors.Add($_.Exception) }
        }
    }
    if ($errors.Count -eq 1) { throw $errors[0] }
    if ($errors.Count -gt 1) {
        throw [AggregateException]::new("Controller '$Package' compilation and cleanup failed.", $errors.ToArray())
    }
    return $executable
}

function Get-ReleaseBinariesExecutable {
    [CmdletBinding()]
    [OutputType([string])]
    param()

    Get-ReleaseControllerExecutable -Package release-binaries
}

function Invoke-ReleaseBinariesHelper {
    [CmdletBinding()]
    [OutputType([string])]
    param(
        [Parameter(Mandatory)][ValidateSet('plan', 'run')][string] $Operation,
        [Parameter(Mandatory)][string] $InputJson,
        [Parameter(Mandatory)][string] $Repository,
        [string] $OutputPath,
        [switch] $NoUpload
    )

    $executable = Get-ReleaseBinariesExecutable
    $inputPath = (New-TemporaryFile).FullName
    $errors = [Collections.Generic.List[Exception]]::new()
    $output = @()
    try {
        Set-Content -LiteralPath $inputPath -Value $InputJson -Encoding utf8NoBOM
        $arguments = @($Operation, '--input', $inputPath, '--repository', $Repository)
        if ($Operation -eq 'run') {
            if ([string]::IsNullOrWhiteSpace($OutputPath)) {
                throw 'Batch execution requires an output directory.'
            }
            $controller = (Resolve-Path (Join-Path $PSScriptRoot '..' '..')).Path
            $arguments += @('--controller', $controller, '--output', $OutputPath)
            if ($NoUpload) { $arguments += '--no-upload' }
        } elseif ($NoUpload -or $OutputPath) {
            throw 'Plan does not accept run-only options.'
        }
        $output = @(& $executable @arguments)
        if ($LASTEXITCODE -ne 0) { throw "release-binaries $Operation failed (exit $LASTEXITCODE)." }
    } catch {
        $errors.Add($_.Exception)
    } finally {
        try { Remove-Item -LiteralPath $inputPath -Force } catch { $errors.Add($_.Exception) }
    }
    if ($errors.Count -eq 1) { throw $errors[0] }
    if ($errors.Count -gt 1) {
        throw [AggregateException]::new("Controller invocation and cleanup of '$inputPath' failed.", $errors.ToArray())
    }
    return $output -join "`n"
}

Export-ModuleMember -Function Invoke-ReleaseBinariesHelper, Get-ReleaseControllerExecutable
