#requires -Version 7

# Thin native-helper boundary shared by release planning and the platform build job.
# Rust owns grouping, asset completeness and batch execution; PowerShell owns controller
# compilation and structured temporary inputs. No helper is compiled from a released source.
# Ref: packages/release-binaries/docs/implementation.md.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

Import-Module (Join-Path $PSScriptRoot '..' 'build' 'CargoExecutable.psm1') -Force

function Get-ReleaseBinariesExecutable {
    [CmdletBinding()]
    [OutputType([string])]
    param()

    Push-Location (Join-Path $PSScriptRoot '..' '..')
    $tokens = @{}
    foreach ($name in @('GH_TOKEN', 'GITHUB_TOKEN', 'GIT_TOKEN', 'INPUT_TOKEN', 'DEFAULT_GITHUB_TOKEN')) {
        $tokens[$name] = [Environment]::GetEnvironmentVariable($name)
        [Environment]::SetEnvironmentVariable($name, $null)
    }
    try {
        $messages = @(cargo build --package release-binaries --locked --message-format=json-render-diagnostics)
        return Resolve-CargoExecutable -CargoMessage $messages -TargetName 'release-binaries'
    } finally {
        Pop-Location
        foreach ($name in $tokens.Keys) {
            [Environment]::SetEnvironmentVariable($name, $tokens[$name])
        }
    }
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
    $inputPath = [IO.Path]::GetTempFileName()
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
        return $output -join "`n"
    } finally {
        Remove-Item -LiteralPath $inputPath -Force
    }
}

Export-ModuleMember -Function Invoke-ReleaseBinariesHelper
