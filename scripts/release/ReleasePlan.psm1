#requires -Version 7.6

# Version-readiness and compatibility gates called by justfiles/just_release.just from local
# validation, Standard validation and merge-queue validation. Rust owns release decisions and
# target selection; this module invokes Cargo, emits CI targets and checks compatibility exits.
# Version planning uses the self-contained increment-versions skill or the documented CLI.
# Ref: docs/build-and-tooling.md, "Automation language and boundaries", and
# .github/workflows/implementation.md.
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true

function Get-ReleasePlanCargoArgument {
    # Validation builds the helper without changing the reviewed lockfile.
    [CmdletBinding()]
    [OutputType([string[]])]
    param(
        [Parameter(Mandatory)][string[]] $Command,
        [string] $Base
    )

    $argument = @('run', '-p', 'cargo-release-plan', '--locked', '--') + $Command
    if (-not [string]::IsNullOrWhiteSpace($Base)) {
        $argument += @('--base', $Base)
    }
    return $argument
}

function Invoke-ReleasePlanCargo {
    # Check native status before exposing output, including with injected Cargo implementations.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string[]] $Command,
        [Parameter(Mandatory)][scriptblock] $Cargo,
        [string] $Base
    )

    $argument = Get-ReleasePlanCargoArgument -Command $Command -Base $Base
    $output = & $Cargo $argument
    if ($LASTEXITCODE -ne 0) {
        throw "cargo-release-plan $($Command[0]) failed with exit code $LASTEXITCODE."
    }
    return $output
}

function Get-ReleasePlanJson {
    # Keep a JSON array intact across every PowerShell function boundary, including [] and [x].
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string[]] $Command,
        [Parameter(Mandatory)][scriptblock] $Cargo
    )

    $json = Invoke-ReleasePlanCargo -Command $Command -Cargo $Cargo
    return , (ConvertFrom-Json -InputObject ($json -join "`n") -NoEnumerate)
}

function Write-ReleasePlanBaseVerbose {
    [CmdletBinding()]
    param([string] $Base)

    if (-not [string]::IsNullOrWhiteSpace($Base)) {
        Write-Verbose "Using RELEASE_PLAN_BASE=$Base as the explicit release baseline." -Verbose
    } else {
        Write-Verbose 'Using the release baseline selected by cargo-release-plan.' -Verbose
    }
}

function Get-AffectedSemverCheckTarget {
    [CmdletBinding()]
    [OutputType([string[]])]
    param(
        [Parameter(Mandatory)][string] $ReportPath,
        [scriptblock] $Cargo = { param([string[]] $Argument) & cargo @Argument }
    )

    return , (Get-ReleasePlanJson `
        -Command @('semver-targets', '--report', $ReportPath, '--verbose') -Cargo $Cargo)
}

function Get-SemverCheckCargoArgument {
    param(
        [Parameter(Mandatory)][string[]] $Package
    )

    $argument = @('semver-checks', '--all-features')
    foreach ($name in $Package) {
        $argument += @('-p', $name)
    }
    return $argument
}

function Get-SemverCheckTargetDirectory {
    # A short workspace-specific Windows path avoids MSVC MAX_PATH failures in the generated
    # placeholder workspaces. Reassess when cargo-semver-checks shortens its generated paths:
    # https://github.com/obi1kenobi/cargo-semver-checks/issues/1725
    [CmdletBinding()]
    [OutputType([string])]
    param(
        [string] $WorkspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path,
        [string] $TempRoot = [IO.Path]::GetTempPath()
    )

    $workspacePath = [IO.Path]::GetFullPath($WorkspaceRoot)
    $hash = [Security.Cryptography.SHA256]::HashData(
        [Text.Encoding]::UTF8.GetBytes($workspacePath)
    )
    # Cache identities need collision resistance while conserving path budget.
    $workspaceIdLength = 16
    $workspaceId = [Convert]::ToHexString($hash).Substring(0, $workspaceIdLength).ToLowerInvariant()
    $candidate = Join-Path ([IO.Path]::GetFullPath($TempRoot)) "fsc-$workspaceId"
    $configured = [Environment]::GetEnvironmentVariable('CARGO_TARGET_DIR', 'Process')
    if (-not [string]::IsNullOrWhiteSpace($configured)) {
        $configuredPath = [IO.Path]::GetFullPath($configured)
        if ($configuredPath.Length -lt $candidate.Length) {
            return $configuredPath
        }
    }
    return $candidate
}

function Invoke-WithSemverCheckTargetDirectory {
    # Only cargo-semver-checks receives the short target root; other Cargo commands keep theirs.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][scriptblock] $Action,
        [AllowNull()][string] $TargetDirectory
    )

    if (-not $PSBoundParameters.ContainsKey('TargetDirectory') -and $IsWindows) {
        $TargetDirectory = Get-SemverCheckTargetDirectory
    }
    if ([string]::IsNullOrWhiteSpace($TargetDirectory)) {
        & $Action
        return
    }
    $previousTargetDirectory =
        [Environment]::GetEnvironmentVariable('CARGO_TARGET_DIR', 'Process')
    try {
        Write-Verbose (
            "Using CARGO_TARGET_DIR=$TargetDirectory for cargo-semver-checks because its " +
            'generated build paths can exceed the Windows path limit.'
        ) -Verbose
        [Environment]::SetEnvironmentVariable('CARGO_TARGET_DIR', $TargetDirectory, 'Process')
        & $Action
    } finally {
        # The provider removes null instead of leaving an empty variable that Cargo rejects.
        $env:CARGO_TARGET_DIR = $previousTargetDirectory
    }
}

function Invoke-SemverCheckCargo {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string[]] $Argument,
        [scriptblock] $Cargo = { param([string[]] $CargoArgument) & cargo @CargoArgument },
        [AllowNull()][string] $TargetDirectory
    )

    $action = { & $Cargo $Argument }
    if ($PSBoundParameters.ContainsKey('TargetDirectory')) {
        Invoke-WithSemverCheckTargetDirectory -Action $action -TargetDirectory $TargetDirectory
    } else {
        Invoke-WithSemverCheckTargetDirectory -Action $action
    }
}

function Invoke-VerifySemverCheck {
    [CmdletBinding()]
    param([scriptblock] $Cargo = { param([string[]] $Argument) & cargo @Argument })

    # A small package keeps the canary cheap. --baseline-rev fixes only the baseline; editing
    # this package can produce a genuine finding rather than an infrastructure error.
    $package = 'folo_utils'
    Write-Verbose "Verifying cargo-semver-checks against HEAD for canary '$package'." -Verbose
    try {
        Invoke-SemverCheckCargo `
            -Argument @('semver-checks', '--baseline-rev', 'HEAD', '-p', $package) -Cargo $Cargo
        if ($LASTEXITCODE -ne 0) {
            throw "cargo-semver-checks canary failed with exit code $LASTEXITCODE."
        }
    } catch {
        Write-Host (
            "cargo-semver-checks could not complete its '$package' canary. A broken tool is " +
            'not evidence of compatibility. Check for work-tree edits to the canary package; ' +
            "otherwise update with 'cargo install cargo-semver-checks --locked' and retry."
        ) -ForegroundColor Red
        throw
    }
}

function Invoke-SemverCheck {
    [CmdletBinding()]
    param(
        [AllowEmptyString()][string] $Package,
        [scriptblock] $Cargo = { param([string[]] $Argument) & cargo @Argument }
    )

    $targets = @($Package -split '\s+' | Where-Object { $_ })
    if ($targets.Count -eq 0) {
        Write-Host 'No consumer-contract packages require cargo-semver-checks; skipping.'
        return
    }
    Invoke-SemverCheckCargo -Argument (Get-SemverCheckCargoArgument -Package $targets) -Cargo $Cargo
    if ($LASTEXITCODE -ne 0) {
        throw "cargo-semver-checks failed with exit code $LASTEXITCODE."
    }
}

function Invoke-ValidateVersions {
    # Publish report-selected CI targets alongside the version-readiness verdict.
    [Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSUseSingularNouns', '',
        Justification = 'Names the plural just validate-versions entry point.')]
    [CmdletBinding()]
    param(
        [string] $GitHubOutputPath = $env:GITHUB_OUTPUT,
        [string] $Base = $env:RELEASE_PLAN_BASE,
        [scriptblock] $Cargo = { param([string[]] $Argument) & cargo @Argument }
    )

    Write-ReleasePlanBaseVerbose -Base $Base
    if (-not [string]::IsNullOrWhiteSpace($GitHubOutputPath)) {
        Import-Module (Join-Path $PSScriptRoot 'ReleaseAutomation.psm1') -Force
        $outDir = Join-Path 'target' "release-plan-$(New-Guid)"
        New-Item -ItemType Directory -Path $outDir -Force | Out-Null
        try {
            Invoke-ReleasePlanCargo -Command @('report', '--out-dir', $outDir) `
                -Base $Base -Cargo $Cargo
            $targets = Get-AffectedSemverCheckTarget `
                -ReportPath (Join-Path $outDir 'report.json') -Cargo $Cargo
            $previousOutput = $env:GITHUB_OUTPUT
            try {
                $env:GITHUB_OUTPUT = $GitHubOutputPath
                Set-GitHubOutput -Name semver_targets -Value ($targets -join ' ') -AllowEmptyValue
            } finally {
                $env:GITHUB_OUTPUT = $previousOutput
            }
        } finally {
            Remove-Item -LiteralPath $outDir -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
    Invoke-ReleasePlanCargo -Command @('check', '--format', 'github') -Base $Base -Cargo $Cargo
}

Export-ModuleMember -Function Invoke-ValidateVersions, Invoke-VerifySemverCheck, Invoke-SemverCheck
