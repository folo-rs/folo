#requires -Version 7.6

# Version-readiness and compatibility gates called by justfiles/just_release.just from local
# validation, Standard validation and Merge queue validation. Rust owns release decisions and
# target selection; this module freezes the CLI's history/target context, emits CI targets and
# checks compatibility exit codes. Only Rust interprets release ancestry and anticipated targets.
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
        [string] $History,
        [string] $MergeTarget
    )

    $argument = @('run', '-p', 'cargo-release-plan', '--locked', '--') + $Command
    if (-not [string]::IsNullOrWhiteSpace($History)) {
        $argument += @('--release-history', $History)
    }
    if (-not [string]::IsNullOrWhiteSpace($MergeTarget)) {
        $argument += @('--merge-target', $MergeTarget)
    }
    return $argument
}

function Invoke-ReleasePlanCargo {
    # Check native status before exposing output, including with injected Cargo implementations.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string[]] $Command,
        [Parameter(Mandatory)][scriptblock] $Cargo,
        [string] $History,
        [string] $MergeTarget
    )

    $argument = Get-ReleasePlanCargoArgument -Command $Command -History $History -MergeTarget $MergeTarget
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
        [Parameter(Mandatory)][scriptblock] $Cargo,
        [string] $History,
        [string] $MergeTarget
    )

    $json = Invoke-ReleasePlanCargo -Command $Command -Cargo $Cargo -History $History -MergeTarget $MergeTarget
    return , (ConvertFrom-Json -InputObject ($json -join "`n") -NoEnumerate)
}

function Get-ReleasePlanContext {
    # The tool owns configured-branch discovery, target normalization and ancestry validation.
    # Validate only the consumed handoff shape here; never derive an anchor or target in PowerShell.
    [CmdletBinding()]
    param(
        [string] $History,
        [string] $Base,
        [string] $MergeTarget,
        [Parameter(Mandatory)][scriptblock] $Cargo
    )

    if (-not [string]::IsNullOrWhiteSpace($Base)) {
        if (-not [string]::IsNullOrWhiteSpace($History) -and $History -cne $Base) {
            throw 'RELEASE_PLAN_HISTORY and its legacy RELEASE_PLAN_BASE alias must not select different release histories.'
        }
        $History = $Base
    }
    if ([string]::IsNullOrWhiteSpace($History) -and
        [string]::IsNullOrWhiteSpace($MergeTarget) -and $env:GITHUB_ACTIONS -cne 'true') {
        # This is Folo's local readiness wrapper, whose release branch is main. Preserve local
        # history resolution without fetching; explicit or hosted contexts retain their own policy.
        $History = 'origin/main'
    }
    $context = Get-ReleasePlanJson -Command @('release-context') -History $History `
        -MergeTarget $MergeTarget -Cargo $Cargo
    if ($context -isnot [pscustomobject] -or
        @('schema_version', 'release_history', 'merge_target' |
            Where-Object { $_ -cnotin $context.PSObject.Properties.Name }).Count -gt 0) {
        throw 'release-context must return an object with explicit schema_version, release_history and merge_target fields.'
    }
    if (($context.schema_version -isnot [long] -and $context.schema_version -isnot [int]) -or
        $context.schema_version -ne 2 -or
        $context.release_history -isnot [string] -or
        $context.release_history -cnotmatch '^[0-9a-f]{40}$' -or
        ($null -ne $context.merge_target -and
            ($context.merge_target -isnot [string] -or $context.merge_target -cnotmatch '^[0-9a-f]{40}$'))) {
        throw 'release-context schema 2 must supply a full release-history commit and a null or full normalized merge-target commit.'
    }
    $target = if ($null -eq $context.merge_target) { 'none' } else { $context.merge_target }
    Write-Verbose "Release assessment uses actual history $($context.release_history) and normalized merge target $target." -Verbose
    return $context
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
        [string] $History = $env:RELEASE_PLAN_HISTORY,
        [string] $Base = $env:RELEASE_PLAN_BASE,
        [string] $MergeTarget = $env:RELEASE_PLAN_MERGE_TARGET,
        [scriptblock] $Cargo = { param([string[]] $Argument) & cargo @Argument }
    )

    $context = Get-ReleasePlanContext -History $History -Base $Base -MergeTarget $MergeTarget -Cargo $Cargo
    if (-not [string]::IsNullOrWhiteSpace($GitHubOutputPath)) {
        Import-Module (Join-Path $PSScriptRoot 'ReleaseAutomation.psm1') -Force
        $outDir = Join-Path 'target' "release-plan-$(New-Guid)"
        New-Item -ItemType Directory -Path $outDir -Force | Out-Null
        try {
            Invoke-ReleasePlanCargo -Command @('report', '--out-dir', $outDir) `
                -History $context.release_history -MergeTarget $context.merge_target -Cargo $Cargo
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
    Invoke-ReleasePlanCargo -Command @('check', '--format', 'github') `
        -History $context.release_history -MergeTarget $context.merge_target -Cargo $Cargo
}

Export-ModuleMember -Function Invoke-ValidateVersions, Invoke-VerifySemverCheck, Invoke-SemverCheck
