#requires -Version 7

# Helpers for pull-request release-plan generation and application.
#
# `cargo-release-plan` owns released-content comparison. This module validates its report,
# selects the explicitly supported cargo-semver-checks targets, and provides the deterministic
# mechanics used by the increment-versions skill. See `.github/workflows/implementation.md`.

Set-StrictMode -Version Latest

# Must match packages/cargo-release-plan/src/plan.rs. An incompatible report must fail closed.
$script:ReleasePlanSchemaVersion = [long] 1

# Local working-file format used by the increment-versions skill.
$script:ChangeDecisionSchemaVersion = [long] 1

# Packages whose library surface is a supported consumer contract. Implementation partitions,
# test-support packages, and undocumented handoff crates are intentionally absent. A change in a
# grouped implementation package selects the listed public package from that group.
$script:SemverCheckPackage = [System.Collections.Generic.HashSet[string]]::new(
    [System.StringComparer]::Ordinal
)
@(
    'all_the_time'
    'alloc_tracker'
    'awaiter_set'
    'cargo-bench-history'
    'cargo-detect-package'
    'cargo-freeze-deps'
    'cargo-release-plan'
    'cpulist'
    'dure'
    'events'
    'events_once'
    'fast_time'
    'future_deque'
    'infinity_pool'
    'linked'
    'many_cpus'
    'many_cpus_benchmarking'
    'nm'
    'nm_otel'
    'par_bench'
    'region_cached'
    'region_local'
    'vicinal'
) | ForEach-Object { [void] $script:SemverCheckPackage.Add($_) }

function Get-ReleasePlanCargoArgument {
    # Argument vector for `cargo run -p cargo-release-plan --locked -- ...`. Forwards
    # `$Base` as `--base` when set; otherwise the tool chooses the release baseline.
    [CmdletBinding()]
    [OutputType([string[]])]
    param(
        [Parameter(Mandatory)][string[]] $Command,
        [string] $Base = $env:RELEASE_PLAN_BASE
    )

    $argument = @('run', '-p', 'cargo-release-plan', '--locked', '--') + $Command
    if (-not [string]::IsNullOrWhiteSpace($Base)) {
        $argument += @('--base', $Base)
    }
    return $argument
}

function Write-ReleasePlanBaseVerbose {
    # Explanatory note for the release baseline this invocation uses.
    [CmdletBinding()]
    param(
        [string] $Base = $env:RELEASE_PLAN_BASE
    )

    if (-not [string]::IsNullOrWhiteSpace($Base)) {
        Write-Verbose "Using RELEASE_PLAN_BASE=$Base as the release baseline (explicit)" -Verbose
    } else {
        Write-Verbose 'Using the release baseline selected by cargo-release-plan' -Verbose
    }
}

function Read-ReleasePlanReport {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $ReportPath
    )

    if (-not (Test-Path -LiteralPath $ReportPath)) {
        throw "release-plan report not found at '$ReportPath'."
    }

    $report = Get-Content -LiteralPath $ReportPath -Raw | ConvertFrom-Json
    if ($null -eq $report -or $report -is [System.Array]) {
        throw "release-plan report at '$ReportPath' must be a JSON object."
    }

    $field = @($report.PSObject.Properties.Name)
    if ($field -notcontains 'schema_version' -or $null -eq $report.schema_version) {
        throw "release-plan report at '$ReportPath' is missing schema_version."
    }
    if (($report.schema_version -isnot [long] -and $report.schema_version -isnot [int]) -or
        [long] $report.schema_version -ne $script:ReleasePlanSchemaVersion) {
        throw "release-plan report at '$ReportPath' uses unsupported schema_version '$($report.schema_version)'; expected $script:ReleasePlanSchemaVersion."
    }
    if ($field -notcontains 'packages' -or $report.packages -isnot [System.Array]) {
        throw "release-plan report at '$ReportPath' packages must be an array."
    }
    if ($field -notcontains 'groups' -or $null -eq $report.groups -or
        $report.groups -is [System.Array]) {
        throw "release-plan report at '$ReportPath' groups must be an object."
    }

    $seen = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($package in $report.packages) {
        if ($null -eq $package) {
            throw "release-plan report at '$ReportPath' contains a null package."
        }
        $packageField = @($package.PSObject.Properties.Name)
        foreach ($required in @('name', 'declared_version', 'status', 'changed', 'dependencies')) {
            if ($packageField -notcontains $required) {
                throw "release-plan report at '$ReportPath' package is missing $required."
            }
        }
        $name = [string] $package.name
        if ([string]::IsNullOrWhiteSpace($name) -or -not $seen.Add($name)) {
            throw "release-plan report at '$ReportPath' contains an empty or duplicate package name."
        }
        if ([string]::IsNullOrWhiteSpace([string] $package.declared_version)) {
            throw "release-plan report at '$ReportPath' package '$name' has no declared_version."
        }
        if ([string] $package.status -notin @('needs-increment', 'pending-release', 'unchanged')) {
            throw "release-plan report at '$ReportPath' package '$name' has unsupported status '$($package.status)'."
        }
        if ($package.changed -isnot [System.Array]) {
            throw "release-plan report at '$ReportPath' package '$name' changed must be an array."
        }
        if ($package.dependencies -isnot [System.Array]) {
            throw "release-plan report at '$ReportPath' package '$name' dependencies must be an array."
        }
    }

    foreach ($group in $report.groups.PSObject.Properties) {
        if ($null -eq $group.Value -or
            $group.Value.PSObject.Properties.Name -notcontains 'members' -or
            $group.Value.members -isnot [System.Array]) {
            throw "release-plan report at '$ReportPath' group '$($group.Name)' members must be an array."
        }
    }

    return $report
}

function Get-PackageByName {
    param(
        [Parameter(Mandatory)] $Report
    )

    $byName = [ordered]@{}
    foreach ($package in $Report.packages) {
        $byName[[string] $package.name] = $package
    }
    return $byName
}

function Get-SemverCheckPackage {
    # Returns the explicit consumer-contract targets affected by a release-plan report. A
    # well-formed report with no selected target is a valid empty result.
    [CmdletBinding()]
    [OutputType([string[]])]
    param(
        [Parameter(Mandatory)][string] $ReportPath
    )

    $report = Read-ReleasePlanReport -ReportPath $ReportPath
    $selected = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )

    foreach ($package in $report.packages) {
        $status = [string] $package.status
        if ($status -notin @('needs-increment', 'pending-release') -or
            @($package.changed).Count -eq 0) {
            continue
        }

        $candidate = @([string] $package.name)
        if ($package.PSObject.Properties.Name -contains 'group' -and
            -not [string]::IsNullOrWhiteSpace([string] $package.group)) {
            $group = $report.groups.PSObject.Properties[[string] $package.group]
            if ($null -eq $group) {
                throw "release-plan report at '$ReportPath' package '$($package.name)' names unknown group '$($package.group)'."
            }
            $candidate = @($group.Value.members | ForEach-Object { [string] $_ })
        }

        foreach ($name in $candidate) {
            if ($script:SemverCheckPackage.Contains($name)) {
                [void] $selected.Add($name)
            }
        }
    }

    return @($selected | Sort-Object)
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

function Complete-SemverChecksCollect {
    # Exit 0 means the tool could not determine a required version increment. Exit 100 is its
    # documented finding exit and is retained as evidence. Any other code is a tool failure.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][int] $ExitCode,
        [Parameter(Mandatory)][string] $LogPath
    )

    if ($ExitCode -eq 0 -or $ExitCode -eq 100) {
        Write-Host "cargo-semver-checks exited $ExitCode; log written to $LogPath"
        return
    }

    throw "cargo-semver-checks failed with exit $ExitCode (log: $LogPath)."
}

function Invoke-ReleaseReport {
    # Collects the release-plan report and cargo-semver-checks evidence for the same explicit
    # consumer-contract target policy used by CI.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $OutDir,
        [string] $Base = $env:RELEASE_PLAN_BASE,
        [scriptblock] $Cargo = { param([string[]] $Argument) & cargo @Argument }
    )

    if ([string]::IsNullOrWhiteSpace($OutDir)) {
        throw 'release-report requires an output directory.'
    }
    New-Item -ItemType Directory -Path $OutDir -Force | Out-Null

    Write-ReleasePlanBaseVerbose -Base $Base
    $reportArgument =
        Get-ReleasePlanCargoArgument -Command @('report', '--out-dir', $OutDir) -Base $Base
    & $Cargo $reportArgument

    $reportPath = Join-Path $OutDir 'report.json'
    $package = @(Get-SemverCheckPackage -ReportPath $reportPath)
    $logPath = Join-Path $OutDir 'semver-checks.log'
    if ($package.Count -eq 0) {
        'No consumer-contract package requires a cargo-semver-checks comparison.' |
            Set-Content -LiteralPath $logPath -Encoding utf8
        Write-Host "No cargo-semver-checks target; log written to $logPath"
        return
    }

    $argument = Get-SemverCheckCargoArgument -Package $package
    Write-Verbose "Running cargo $($argument -join ' '); output captured at $logPath" -Verbose
    $previousPreference = $PSNativeCommandUseErrorActionPreference
    $PSNativeCommandUseErrorActionPreference = $false
    try {
        & $Cargo $argument 2>&1 | Tee-Object -FilePath $logPath
        $exitCode = $LASTEXITCODE
    } finally {
        $PSNativeCommandUseErrorActionPreference = $previousPreference
    }
    Complete-SemverChecksCollect -ExitCode $exitCode -LogPath $logPath
}

function Invoke-SemverCheck {
    # CI wrapper for one or more space-separated package names.
    [CmdletBinding()]
    param(
        [AllowEmptyString()][string] $Package,
        [scriptblock] $Cargo = { param([string[]] $Argument) & cargo @Argument }
    )

    $name = @($Package -split '\s+' | Where-Object { $_ })
    if ($name.Count -eq 0) {
        Write-Host 'No consumer-contract packages require cargo-semver-checks; skipping.'
        return
    }

    $argument = Get-SemverCheckCargoArgument -Package $name
    Write-Verbose "Running cargo $($argument -join ' ')" -Verbose
    & $Cargo $argument
}

function Invoke-ExpandReleasePlan {
    # Resolves a generated plan's version groups and levels into explicit per-package versions.
    # Group expansion belongs to cargo-release-plan, so the skill presents the tool's own answer
    # rather than a second implementation of the same rules.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $PlanPath,
        [Parameter(Mandatory)][string] $ExpandedPath,
        [scriptblock] $Cargo = { param([string[]] $Argument) & cargo @Argument }
    )

    if ([string]::IsNullOrWhiteSpace($PlanPath)) {
        throw 'expand-release-plan requires a plan JSON path.'
    }
    if (-not (Test-Path -LiteralPath $PlanPath)) {
        throw "expand-release-plan plan file not found: $PlanPath"
    }
    if ([string]::IsNullOrWhiteSpace($ExpandedPath)) {
        throw 'expand-release-plan requires an output path.'
    }

    Write-Verbose "Expanding version groups in $PlanPath via cargo-release-plan expand" -Verbose
    & $Cargo @(
        'run', '-p', 'cargo-release-plan', '--locked', '--',
        'expand', '--plan', $PlanPath, '--out', $ExpandedPath
    )
}

function Invoke-ApplyReleasePlan {
    # Applies a generated cargo-release-plan file after validating the path supplied by the skill.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $PlanPath,
        [scriptblock] $Cargo = { param([string[]] $Argument) & cargo @Argument }
    )

    if ([string]::IsNullOrWhiteSpace($PlanPath)) {
        throw 'apply-release-plan requires a plan JSON path.'
    }
    if (-not (Test-Path -LiteralPath $PlanPath)) {
        throw "apply-release-plan plan file not found: $PlanPath"
    }

    Write-Verbose "Applying increment plan from $PlanPath via cargo-release-plan apply" -Verbose
    & $Cargo @('run', '-p', 'cargo-release-plan', '--locked', '--', 'apply', '--plan', $PlanPath)
}

function Invoke-ValidateVersions {
    # CI orchestration for `just validate-versions`. The report and check use identical inputs.
    [Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSUseSingularNouns', '',
        Justification = 'The function is the just validate-versions recipe body; the job id is plural.')]
    [CmdletBinding()]
    param(
        [string] $GitHubOutputPath = $env:GITHUB_OUTPUT,
        [string] $Base = $env:RELEASE_PLAN_BASE,
        [scriptblock] $Cargo = { param([string[]] $Argument) & cargo @Argument }
    )

    Write-ReleasePlanBaseVerbose -Base $Base

    if (-not [string]::IsNullOrWhiteSpace($GitHubOutputPath)) {
        Import-Module (Join-Path $PSScriptRoot 'ReleaseAutomation.psm1') -Force

        $outDir = Join-Path ([System.IO.Path]::GetTempPath()) "release-plan-$(New-Guid)"
        New-Item -ItemType Directory -Path $outDir | Out-Null
        try {
            $reportArgument =
                Get-ReleasePlanCargoArgument -Command @('report', '--out-dir', $outDir) -Base $Base
            & $Cargo $reportArgument

            $released = @(Get-SemverCheckPackage -ReportPath (Join-Path $outDir 'report.json'))
            $previousOutput = $env:GITHUB_OUTPUT
            $env:GITHUB_OUTPUT = $GitHubOutputPath
            try {
                # The required zero-target representation is a present `released=` output.
                Set-GitHubOutput -Name released -Value ($released -join ' ')
            } finally {
                $env:GITHUB_OUTPUT = $previousOutput
            }
        } finally {
            Remove-Item -LiteralPath $outDir -Recurse -Force -ErrorAction SilentlyContinue
        }
    }

    $checkArgument =
        Get-ReleasePlanCargoArgument -Command @('check', '--format', 'github') -Base $Base
    & $Cargo $checkArgument
}

function Get-ReachablePackageName {
    param(
        [Parameter(Mandatory)][string] $Start,
        [Parameter(Mandatory)][hashtable] $Dependency
    )

    $reachable = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    $pending = [System.Collections.Generic.Stack[string]]::new()
    $pending.Push($Start)
    while ($pending.Count -gt 0) {
        $name = $pending.Pop()
        if (-not $reachable.Add($name)) {
            continue
        }
        foreach ($dependencyName in $Dependency[$name]) {
            $pending.Push($dependencyName)
        }
    }

    # The comma keeps the set intact. PowerShell enumerates a returned collection, so a
    # single-element set would arrive at the caller as a bare string whose `Contains` tests for a
    # substring rather than for membership, silently merging packages whose names share a prefix.
    return , $reachable
}

function Get-ReleasePlanAnalysisBatch {
    # Returns dependency-first analysis batches. Mutually dependent packages share one batch and
    # must be reconsidered together until their decisions stop changing. Property names are the
    # JSON contract the increment-versions skill documents, so they are lower-case.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $ReportPath
    )

    $report = Read-ReleasePlanReport -ReportPath $ReportPath
    $byName = Get-PackageByName -Report $report
    $name = @($byName.Keys | Sort-Object)
    $dependency = @{}
    foreach ($packageName in $name) {
        $dependency[$packageName] = @(
            $byName[$packageName].dependencies |
                ForEach-Object { [string] $_.name } |
                Where-Object { $byName.Contains($_) } |
                Sort-Object -Unique
        )
    }

    $reachable = @{}
    foreach ($packageName in $name) {
        $reachable[$packageName] =
            Get-ReachablePackageName -Start $packageName -Dependency $dependency
    }

    $assigned = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    $component = [System.Collections.Generic.List[object]]::new()
    foreach ($packageName in $name) {
        if ($assigned.Contains($packageName)) {
            continue
        }
        $member = @(
            $name |
                Where-Object {
                    $reachable[$packageName].Contains($_) -and
                    $reachable[$_].Contains($packageName)
                } |
                Sort-Object
        )
        foreach ($memberName in $member) {
            [void] $assigned.Add($memberName)
        }
        $component.Add([pscustomobject]@{
            Id      = $component.Count
            Members = $member
        })
    }

    $componentOf = @{}
    foreach ($entry in $component) {
        foreach ($memberName in $entry.Members) {
            $componentOf[$memberName] = $entry.Id
        }
    }
    $incoming = @{}
    foreach ($entry in $component) {
        $incoming[$entry.Id] = [System.Collections.Generic.HashSet[int]]::new()
        foreach ($memberName in $entry.Members) {
            foreach ($dependencyName in $dependency[$memberName]) {
                $dependencyComponent = [int] $componentOf[$dependencyName]
                if ($dependencyComponent -ne $entry.Id) {
                    [void] $incoming[$entry.Id].Add($dependencyComponent)
                }
            }
        }
    }

    $remaining = [System.Collections.Generic.HashSet[int]]::new()
    foreach ($entry in $component) {
        [void] $remaining.Add($entry.Id)
    }
    $order = 0
    while ($remaining.Count -gt 0) {
        $next = @(
            $remaining |
                Where-Object { $incoming[$_].Count -eq 0 } |
                Sort-Object { $component[$_].Members -join "`0" }
        )
        if ($next.Count -eq 0) {
            throw 'release-plan dependency condensation unexpectedly contains a cycle.'
        }
        foreach ($componentId in $next) {
            $order++
            $members = @($component[$componentId].Members)
            [pscustomobject][ordered]@{
                order    = $order
                packages = $members
                cyclic   = $members.Count -gt 1
            }
            [void] $remaining.Remove($componentId)
            foreach ($otherId in $remaining) {
                [void] $incoming[$otherId].Remove($componentId)
            }
        }
    }
}

function Read-ChangeDecision {
    param(
        [Parameter(Mandatory)][string] $DecisionPath
    )

    if (-not (Test-Path -LiteralPath $DecisionPath)) {
        throw "change-decision file not found at '$DecisionPath'."
    }
    $decision = Get-Content -LiteralPath $DecisionPath -Raw | ConvertFrom-Json
    if ($null -eq $decision -or $decision -is [System.Array]) {
        throw "change-decision file at '$DecisionPath' must be a JSON object."
    }
    $field = @($decision.PSObject.Properties.Name)
    if ($field -notcontains 'schema_version' -or
        ($decision.schema_version -isnot [long] -and
            $decision.schema_version -isnot [int]) -or
        [long] $decision.schema_version -ne $script:ChangeDecisionSchemaVersion) {
        throw "change-decision file at '$DecisionPath' must use schema_version $script:ChangeDecisionSchemaVersion."
    }
    if ($field -notcontains 'changes' -or $decision.changes -isnot [System.Array]) {
        throw "change-decision file at '$DecisionPath' changes must be an array."
    }

    $seen = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($change in $decision.changes) {
        if ($null -eq $change) {
            throw "change-decision file at '$DecisionPath' contains a null change."
        }
        $changeField = @($change.PSObject.Properties.Name)
        if ($changeField.Count -ne 2 -or
            $changeField -notcontains 'name' -or
            $changeField -notcontains 'level') {
            throw "Each change in '$DecisionPath' must contain only name and level."
        }
        $name = [string] $change.name
        if ([string]::IsNullOrWhiteSpace($name) -or -not $seen.Add($name)) {
            throw "Change names in '$DecisionPath' must be non-empty and unique."
        }
        $level = [string] $change.level
        if ($level -cnotin @('breaking', 'nonbreaking', 'patch')) {
            throw "Change '$name' in '$DecisionPath' has unsupported level '$level'."
        }
    }
    return $decision
}

function Read-ExpandedPlan {
    # Reads the per-package names cargo-release-plan resolved a plan into. Group expansion is
    # the tool's, so this only validates the shape it promises.
    param(
        [Parameter(Mandatory)][string] $ExpandedPath
    )

    if (-not (Test-Path -LiteralPath $ExpandedPath)) {
        throw "expanded plan file not found: $ExpandedPath"
    }
    $plan = Get-Content -LiteralPath $ExpandedPath -Raw | ConvertFrom-Json
    if ($null -eq $plan -or $plan -is [System.Array]) {
        throw "expanded plan at '$ExpandedPath' must be a JSON object."
    }
    $field = @($plan.PSObject.Properties.Name)
    if ($field -notcontains 'schema_version' -or
        ($plan.schema_version -isnot [long] -and $plan.schema_version -isnot [int]) -or
        [long] $plan.schema_version -ne $script:ReleasePlanSchemaVersion) {
        throw "expanded plan at '$ExpandedPath' must use schema_version $script:ReleasePlanSchemaVersion."
    }
    if ($field -notcontains 'increments' -or $plan.increments -isnot [System.Array]) {
        throw "expanded plan at '$ExpandedPath' increments must be an array."
    }

    $name = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($increment in $plan.increments) {
        if ($null -eq $increment -or
            $increment.PSObject.Properties.Name -notcontains 'name' -or
            [string]::IsNullOrWhiteSpace([string] $increment.name)) {
            throw "expanded plan at '$ExpandedPath' contains an increment without a name."
        }
        [void] $name.Add([string] $increment.name)
    }
    return @($name | Sort-Object)
}

function Assert-IncrementPackagePublished {
    # Fails unless every package that apply would reach has a confirmed crates.io publication.
    # Reads the expanded plan, so the checked set is exactly the set apply will edit.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $ExpandedPath,
        [scriptblock] $GetPublishStatus = {
            param([string] $Name)
            Get-CratePublishStatus -Name $Name
        }
    )

    Import-Module (Join-Path $PSScriptRoot 'ReleaseAutomation.psm1') -Force
    $package = @(Read-ExpandedPlan -ExpandedPath $ExpandedPath)
    $neverPublished = [System.Collections.Generic.List[string]]::new()
    $unknown = [System.Collections.Generic.List[string]]::new()
    foreach ($name in $package) {
        switch -CaseSensitive (& $GetPublishStatus $name) {
            'Published' { }
            'NeverPublished' { $neverPublished.Add($name) }
            default { $unknown.Add($name) }
        }
    }
    if ($neverPublished.Count -gt 0) {
        $noun = if ($neverPublished.Count -eq 1) { 'package' } else { 'packages' }
        $pronoun = if ($neverPublished.Count -eq 1) { 'it' } else { 'them' }
        throw "The increment reaches never-published $($noun): $($neverPublished -join ', '). First-publish $pronoun manually before applying this plan."
    }
    if ($unknown.Count -gt 0) {
        $noun = if ($unknown.Count -eq 1) { 'package' } else { 'packages' }
        throw "Could not confirm crates.io publication for $($noun): $($unknown -join ', ')."
    }
    Write-Host 'Every package reached by the approved changes is already published.'
}

function Get-MinimumVersionForChange {
    # Lowest version that can carry $Level relative to $Anchor.
    #
    # Cargo treats the leftmost non-zero component as the major component, so a
    # 0.y.z release advances y for a breaking change and z for a compatible one,
    # and a 0.0.z release admits no compatible change at all. Deriving this from
    # the anchor rather than from the level alone keeps 0.x packages, which are
    # most of this workspace, from being systematically over-incremented.
    param(
        [Parameter(Mandatory)][semver] $Anchor,
        [Parameter(Mandatory)][string] $Level
    )

    switch -CaseSensitive ($Level) {
        'breaking' {
            if ($Anchor.Major -eq 0 -and $Anchor.Minor -eq 0) {
                return [semver]::new(0, 0, $Anchor.Patch + 1)
            }
            if ($Anchor.Major -eq 0) {
                return [semver]::new(0, $Anchor.Minor + 1, 0)
            }
            return [semver]::new($Anchor.Major + 1, 0, 0)
        }
        'nonbreaking' {
            if ($Anchor.Major -eq 0) {
                return [semver]::new(0, $Anchor.Minor, $Anchor.Patch + 1)
            }
            return [semver]::new($Anchor.Major, $Anchor.Minor + 1, 0)
        }
        'patch' { return [semver]::new($Anchor.Major, $Anchor.Minor, $Anchor.Patch + 1) }
        default { throw "Unsupported change level '$Level'." }
    }
}

function Get-CargoIncrementLevel {
    param(
        [Parameter(Mandatory)][semver] $Current,
        [Parameter(Mandatory)][semver] $Minimum
    )

    if ($Minimum.Major -gt $Current.Major) {
        return 'major'
    }
    if ($Minimum.Minor -gt $Current.Minor) {
        return 'minor'
    }
    return 'patch'
}

function Get-DecisionGroupName {
    # The key a plan entry folds onto: the package's version group when it has one, otherwise the
    # package itself. Mirrors the tool's own decision keys, so a group counts as already planned
    # whichever member named it.
    param(
        [Parameter(Mandatory)] $ByName,
        [Parameter(Mandatory)][string] $Name
    )

    if (-not $ByName.Contains($Name)) {
        return $Name
    }
    $package = $ByName[$Name]
    if ($package.PSObject.Properties.Name -notcontains 'group' -or
        [string]::IsNullOrWhiteSpace([string] $package.group)) {
        return $Name
    }
    return [string] $package.group
}

function Get-GroupAlignmentIncrement {
    # Exact-version plan entry that puts a drifted group back on one version.
    #
    # Aligning is not an increment: the members simply have to agree, and the highest version any
    # of them already declares is the one they agree on. Raising it instead would publish a new
    # release of every member for no substantive change. The target is the report's own
    # highest-declared-member version, so the rule is not restated here.
    param(
        [Parameter(Mandatory)][string] $Name,
        [Parameter(Mandatory)] $Group
    )

    if ($Group.PSObject.Properties.Name -notcontains 'version' -or
        [string]::IsNullOrWhiteSpace([string] $Group.version)) {
        throw "Group '$Name' has no declared version to align its members on."
    }

    return [ordered]@{
        name    = $Name
        version = [string] $Group.version
    }
}

function New-ReleasePlanFile {
    # Converts approved semantic change levels into cargo-release-plan's mechanical input, and
    # makes every inconsistent version group consistent. Existing pending-release increments are
    # retained and raised only when insufficient.
    [CmdletBinding(SupportsShouldProcess)]
    param(
        [Parameter(Mandatory)][string] $ReportPath,
        [Parameter(Mandatory)][string] $DecisionPath,
        [Parameter(Mandatory)][string] $PlanPath
    )

    $report = Read-ReleasePlanReport -ReportPath $ReportPath
    $decision = Read-ChangeDecision -DecisionPath $DecisionPath
    $byName = Get-PackageByName -Report $report
    $increment = [System.Collections.Generic.List[object]]::new()
    foreach ($change in $decision.changes) {
        $name = [string] $change.name
        if (-not $byName.Contains($name)) {
            throw "Change decision names unknown package '$name'."
        }
        $package = $byName[$name]
        $level = [string] $change.level
        if ($package.PSObject.Properties.Name -notcontains 'anchor' -or
            $null -eq $package.anchor -or
            [string]::IsNullOrWhiteSpace([string] $package.anchor.version)) {
            throw "Package '$name' has no published version anchor and cannot enter an increment plan."
        }
        try {
            $anchor = [semver] [string] $package.anchor.version
            $current = [semver] [string] $package.declared_version
        } catch {
            throw "Package '$name' has an invalid semantic version in the release-plan report."
        }
        # A prerelease version orders below the release it precedes, so the component comparison
        # that derives a Cargo increment level from a minimum version cannot express "drop the
        # prerelease suffix". Rejecting the input keeps a wrong level from being generated
        # silently; every published package in this workspace declares a release version.
        if (-not [string]::IsNullOrEmpty($anchor.PreReleaseLabel) -or
            -not [string]::IsNullOrEmpty($current.PreReleaseLabel)) {
            throw "Package '$name' declares a prerelease version, which this plan generator does not support."
        }
        $minimum = Get-MinimumVersionForChange -Anchor $anchor -Level $level
        if ($current -ge $minimum) {
            continue
        }
        $increment.Add([ordered]@{
            name  = $name
            level = Get-CargoIncrementLevel -Current $current -Minimum $minimum
        })
    }

    # Every version group has to end up on one version, and expansion is plan-driven: a group
    # moves only when an entry names one of its members. The decisions above can easily leave a
    # drifted group unnamed, because no member's content changed or because the decided level was
    # already covered by a pending increment, so the groups no entry reaches are realigned here.
    # Leaving this to a decision instead would make an inconsistent group unrecoverable exactly
    # when its decision is skipped as already sufficient.
    # Ref: packages/cargo-release-plan/docs/design.md, "Version groups".
    $planned = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($entry in $increment) {
        [void] $planned.Add((Get-DecisionGroupName -ByName $byName -Name ([string] $entry.name)))
    }
    foreach ($group in @($report.groups.PSObject.Properties | Sort-Object -Property Name)) {
        if ($group.Value.PSObject.Properties.Name -notcontains 'consistent' -or
            $group.Value.consistent -or
            $planned.Contains($group.Name)) {
            continue
        }
        $increment.Add((Get-GroupAlignmentIncrement -Name $group.Name -Group $group.Value))
    }

    if ($PSCmdlet.ShouldProcess($PlanPath, 'write generated cargo-release-plan input')) {
        $parent = Split-Path -Parent $PlanPath
        if (-not [string]::IsNullOrWhiteSpace($parent)) {
            New-Item -ItemType Directory -Path $parent -Force | Out-Null
        }
        [ordered]@{
            schema_version = $script:ReleasePlanSchemaVersion
            increments     = @($increment)
        } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $PlanPath -Encoding utf8
        Write-Host "Wrote cargo-release-plan input to $PlanPath"
    }
}

Export-ModuleMember -Function `
    Get-ReleasePlanCargoArgument, `
    Write-ReleasePlanBaseVerbose, `
    Get-SemverCheckPackage, `
    Complete-SemverChecksCollect, `
    Invoke-ReleaseReport, `
    Invoke-SemverCheck, `
    Invoke-ExpandReleasePlan, `
    Invoke-ApplyReleasePlan, `
    Invoke-ValidateVersions, `
    Get-ReleasePlanAnalysisBatch, `
    Assert-IncrementPackagePublished, `
    New-ReleasePlanFile
