#requires -Version 7

# Helpers for the `just validate-versions` / `just release-report` / `just semver-checks`
# recipes.
#
# Package classification is `cargo release-plan`'s job; this module orchestrates the CI
# wrappers and reads `report.json` to select the subset `cargo-semver-checks` should run
# on. That subset is the consumer-visible packages this change releases: public shells
# (and ungrouped public packages) whose status is `needs-increment` or `pending-release`,
# including a shell whose grouped `_impl` member has released-content changes.
# `doc(hidden)` `_impl` crates are not themselves comparison targets. Pure filter logic
# is Pester-tested without spawning the Rust tool. See
# `.github/workflows/implementation.md`.

Set-StrictMode -Version Latest

function Get-ReleasePlanCargoArgument {
    # Argument vector for `cargo run -p cargo-release-plan --locked -- ...`. Forwards
    # `$Base` (CI's `RELEASE_PLAN_BASE`) as `--base` when set; otherwise the tool
    # chooses the release baseline.
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
        Write-Verbose "Using RELEASE_PLAN_BASE=$Base as the release baseline (explicit; not the tool default)" -Verbose
    } else {
        Write-Verbose 'Using the tool default release baseline because RELEASE_PLAN_BASE is unset' -Verbose
    }
}

function Get-SemverCheckPackage {
    # Returns package names from a `cargo release-plan report` JSON file that the CI
    # `semver-checks` job should pass to `cargo semver-checks --all-features`. Order
    # follows first selection. A well-formed report with an empty selected set is a
    # skip, not an error. A missing schema or packages array is malformed and fails
    # closed so a schema drift cannot be read as "nothing to check".
    [CmdletBinding()]
    [OutputType([string[]])]
    param(
        [Parameter(Mandatory)][string] $ReportPath
    )

    if (-not (Test-Path -LiteralPath $ReportPath)) {
        throw "release-plan report not found at '$ReportPath'."
    }

    $json = Get-Content -LiteralPath $ReportPath -Raw | ConvertFrom-Json
    if ($null -eq $json) {
        throw "release-plan report at '$ReportPath' is empty or not JSON."
    }

    $field = @($json.PSObject.Properties.Name)
    if ($field -notcontains 'schema_version' -or $null -eq $json.schema_version) {
        throw "release-plan report at '$ReportPath' is missing schema_version."
    }
    if ($field -notcontains 'packages' -or $null -eq $json.packages) {
        throw "release-plan report at '$ReportPath' is missing packages."
    }

    $byName = [ordered]@{}
    foreach ($entry in @($json.packages)) {
        if ($null -eq $entry) { continue }
        $name = ''
        if ($entry.PSObject.Properties.Name -contains 'name' -and $null -ne $entry.name) {
            $name = [string] $entry.name
        }
        if ([string]::IsNullOrWhiteSpace($name)) { continue }
        $byName[$name] = $entry
    }

    $groupMember = @{}
    if ($field -contains 'groups' -and $null -ne $json.groups) {
        foreach ($group in $json.groups.PSObject.Properties) {
            $member = @()
            if ($null -ne $group.Value -and
                ($group.Value.PSObject.Properties.Name -contains 'members') -and
                $null -ne $group.Value.members) {
                $member = @($group.Value.members | ForEach-Object { [string] $_ })
            }
            $groupMember[$group.Name] = $member
        }
    }

    $selected = [System.Collections.Generic.List[string]]::new()
    $seen = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)

    foreach ($entry in @($json.packages)) {
        if ($null -eq $entry) { continue }

        $name = ''
        if ($entry.PSObject.Properties.Name -contains 'name' -and $null -ne $entry.name) {
            $name = [string] $entry.name
        }
        if ([string]::IsNullOrWhiteSpace($name)) { continue }

        $status = ''
        if ($entry.PSObject.Properties.Name -contains 'status' -and $null -ne $entry.status) {
            $status = [string] $entry.status
        }
        $releasing = $status -eq 'needs-increment' -or $status -eq 'pending-release'
        if (-not $releasing) { continue }

        $changedCount = 0
        if ($entry.PSObject.Properties.Name -contains 'changed' -and $null -ne $entry.changed) {
            $changedCount = @($entry.changed).Count
        }
        $hasChange = $changedCount -gt 0
        $isImpl = $name.EndsWith('_impl', [System.StringComparison]::Ordinal)

        if (-not $isImpl -and $hasChange) {
            if ($seen.Add($name)) { $selected.Add($name) }
            continue
        }

        if (-not ($isImpl -and $hasChange)) { continue }

        $shell = [System.Collections.Generic.List[string]]::new()
        $groupName = ''
        if ($entry.PSObject.Properties.Name -contains 'group' -and $null -ne $entry.group) {
            $groupName = [string] $entry.group
        }
        if (-not [string]::IsNullOrWhiteSpace($groupName) -and $groupMember.ContainsKey($groupName)) {
            foreach ($memberName in @($groupMember[$groupName])) {
                if (-not $memberName.EndsWith('_impl', [System.StringComparison]::Ordinal)) {
                    $shell.Add($memberName)
                }
            }
        }
        if ($shell.Count -eq 0) {
            $stripped = $name.Substring(0, $name.Length - '_impl'.Length)
            if (-not [string]::IsNullOrWhiteSpace($stripped)) {
                $shell.Add($stripped)
            }
        }

        foreach ($shellName in $shell) {
            if ($byName.Contains($shellName) -and $seen.Add($shellName)) {
                $selected.Add($shellName)
            }
        }
    }

    return @($selected)
}

function Complete-SemverChecksCollect {
    # `just release-report` captures `cargo semver-checks` output as an increment floor.
    # Exit 0 is a clean comparison; 100 is the documented finding-exit (a floor, not a
    # broken tool). Any other code is a tool failure and must not be read as "no floor".
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

function Invoke-ValidateVersions {
    # CI orchestration for `just validate-versions`. When a GitHub output file is
    # present, writes a report from the same inputs the subsequent check uses, emits
    # the `released` list for `semver-checks`, then runs `check --format github`.
    # `report` and `check` each classify; they share one implementation in
    # cargo-release-plan. Combining them would be a CLI change on that tool.
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
        $reportArgument = Get-ReleasePlanCargoArgument -Command @('report', '--out-dir', $outDir) -Base $Base
        & $Cargo $reportArgument

        $released = @(Get-SemverCheckPackage -ReportPath (Join-Path $outDir 'report.json'))
        # Empty join is `released=` (present, empty). semver-checks treats that as a skip.
        $previousOutput = $env:GITHUB_OUTPUT
        $env:GITHUB_OUTPUT = $GitHubOutputPath
        try {
            Set-GitHubOutput -Name released -Value ($released -join ' ')
        } finally {
            $env:GITHUB_OUTPUT = $previousOutput
        }
    }

    $checkArgument = Get-ReleasePlanCargoArgument -Command @('check', '--format', 'github') -Base $Base
    & $Cargo $checkArgument
}

Export-ModuleMember -Function `
    Get-ReleasePlanCargoArgument, `
    Write-ReleasePlanBaseVerbose, `
    Get-SemverCheckPackage, `
    Complete-SemverChecksCollect, `
    Invoke-ValidateVersions
