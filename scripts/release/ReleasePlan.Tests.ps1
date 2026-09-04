#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Pester suite for ReleasePlan.psm1.

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'ReleasePlan.psm1') -Force

    function Get-TestPackage {
        param(
            [Parameter(Mandatory)][string] $Name,
            [string] $Status = 'unchanged',
            [object[]] $Changed = @(),
            [object[]] $Dependencies = @(),
            [string] $Group,
            [string] $DeclaredVersion = '1.0.0',
            [string] $AnchorVersion = '1.0.0'
        )

        $package = [ordered]@{
            name             = $Name
            declared_version = $DeclaredVersion
            status           = $Status
            changed          = @($Changed)
            dependencies     = @($Dependencies)
        }
        if ($PSBoundParameters.ContainsKey('Group')) {
            $package.group = $Group
        }
        if ($PSBoundParameters.ContainsKey('AnchorVersion')) {
            $package.anchor = @{ commit = 'abc123'; version = $AnchorVersion }
        }
        return $package
    }

    function Write-TestReport {
        param(
            [Parameter(Mandatory)][string] $Path,
            [Parameter(Mandatory)][AllowEmptyCollection()][object[]] $Package,
            [hashtable] $Group = @{},
            [long] $SchemaVersion = 1
        )

        [ordered]@{
            schema_version = $SchemaVersion
            packages       = @($Package)
            groups         = $Group
        } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $Path -Encoding utf8
    }

    function Write-TestDecision {
        param(
            [Parameter(Mandatory)][string] $Path,
            [Parameter(Mandatory)][AllowEmptyCollection()][object[]] $Change
        )

        [ordered]@{
            schema_version = 1
            changes        = @($Change)
        } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $Path -Encoding utf8
    }
}

Describe 'Get-ReleasePlanCargoArgument' {
    It 'forwards --base when a release baseline is set' {
        $argument =
            Get-ReleasePlanCargoArgument -Command @('check', '--format', 'github') -Base 'abc123'
        $argument | Should -Contain '--base'
        $argument | Should -Contain 'abc123'
    }

    It 'omits --base when the release baseline is empty' {
        $argument = Get-ReleasePlanCargoArgument -Command @('check') -Base ''
        $argument | Should -Not -Contain '--base'
    }
}

Describe 'Get-SemverCheckPackage' {
    It 'includes supported needs-increment and pending-release packages' {
        $path = Join-Path $TestDrive 'report.json'
        Write-TestReport -Path $path -Package @(
            Get-TestPackage -Name 'events' -Status 'needs-increment' `
                -Changed @(@{ path = 'src/lib.rs' })
            Get-TestPackage -Name 'nm' -Status 'pending-release' `
                -Changed @(@{ path = 'src/lib.rs' })
        )
        Get-SemverCheckPackage -ReportPath $path | Should -Be @('events', 'nm')
    }

    It 'excludes unchanged packages and unsupported handoff crates' {
        $path = Join-Path $TestDrive 'unsupported.json'
        Write-TestReport -Path $path -Package @(
            Get-TestPackage -Name 'events'
            Get-TestPackage -Name 'folo_utils' -Status 'needs-increment' `
                -Changed @(@{ path = 'src/lib.rs' })
        )
        Get-SemverCheckPackage -ReportPath $path | Should -BeNullOrEmpty
    }

    It 'selects the public package when a grouped implementation package changes' {
        $path = Join-Path $TestDrive 'impl.json'
        Write-TestReport -Path $path -Package @(
            Get-TestPackage -Name 'nm_impl' -Status 'needs-increment' -Group 'nm' `
                -Changed @(@{ path = 'src/lib.rs' })
            Get-TestPackage -Name 'nm' -Group 'nm'
        ) -Group @{
            nm = @{ members = @('nm', 'nm_impl'); consistent = $true; version = '1.0.0' }
        }
        Get-SemverCheckPackage -ReportPath $path | Should -Be @('nm')
    }

    It 'maps the real cargo-bench-history private group to only its public package' {
        $path = Join-Path $TestDrive 'cbh.json'
        $members = @('cargo-bench-history', 'cargo-bench-history-faker', 'cbh_stats')
        Write-TestReport -Path $path -Package @(
            Get-TestPackage -Name 'cargo-bench-history' -Group 'cargo-bench-history'
            Get-TestPackage -Name 'cargo-bench-history-faker' -Status 'pending-release' `
                -Group 'cargo-bench-history' -Changed @(@{ path = 'src/lib.rs' })
            Get-TestPackage -Name 'cbh_stats' -Status 'needs-increment' `
                -Group 'cargo-bench-history' -Changed @(@{ path = 'src/lib.rs' })
        ) -Group @{
            'cargo-bench-history' = @{
                members = $members
                consistent = $true
                version = '1.0.0'
            }
        }
        Get-SemverCheckPackage -ReportPath $path | Should -Be @('cargo-bench-history')
    }

    It 'guards explicit targets against ungrouped documented-package drift' {
        # Pin the workspace manifest: the test reads real workspace metadata, and the runner's
        # working directory is not guaranteed to be the workspace root.
        $manifest = Join-Path $PSScriptRoot '../../Cargo.toml'
        InModuleScope ReleasePlan -Parameters @{ Manifest = $manifest } {
            param($Manifest)

            $metadata = cargo metadata --no-deps --format-version 1 --manifest-path $Manifest |
                ConvertFrom-Json
            $grouped = [System.Collections.Generic.HashSet[string]]::new(
                [System.StringComparer]::Ordinal
            )
            foreach ($group in $metadata.metadata.'release-plan'.groups.PSObject.Properties) {
                foreach ($member in $group.Value) {
                    [void] $grouped.Add([string] $member)
                }
            }

            $published = @(
                $metadata.packages |
                    Where-Object { $null -eq $_.publish -or @($_.publish).Count -gt 0 }
            )
            $missing = @(
                $published |
                    Where-Object {
                        -not $grouped.Contains([string] $_.name) -and
                        @(
                            $_.targets |
                                Where-Object {
                                    $_.doc -and
                                    ($_.kind -contains 'lib' -or $_.kind -contains 'proc-macro')
                                }
                        ).Count -gt 0 -and
                        -not $script:SemverCheckPackage.Contains([string] $_.name)
                    } |
                    ForEach-Object { [string] $_.name }
            )
            $stale = @(
                $script:SemverCheckPackage |
                    Where-Object { [string] $_ -notin @($published.name) }
            )

            $missing | Should -BeNullOrEmpty
            $stale | Should -BeNullOrEmpty
        }
    }

    It 'fails closed on an unsupported schema revision' {
        $path = Join-Path $TestDrive 'future.json'
        Write-TestReport -Path $path -Package @() -SchemaVersion 2
        { Get-SemverCheckPackage -ReportPath $path } |
            Should -Throw '*unsupported schema_version*expected 1*'
    }

    It 'fails closed when packages is not an array' {
        $path = Join-Path $TestDrive 'object.json'
        '{"schema_version":1,"packages":{"name":"events"},"groups":{}}' |
            Set-Content -LiteralPath $path -Encoding utf8
        { Get-SemverCheckPackage -ReportPath $path } |
            Should -Throw '*packages must be an array*'
    }

    It 'joins an empty selected set to the required released= representation' {
        $path = Join-Path $TestDrive 'empty.json'
        Write-TestReport -Path $path -Package @(
            Get-TestPackage -Name 'events'
        )
        $released = @(Get-SemverCheckPackage -ReportPath $path)
        $released.Count | Should -Be 0
        ($released -join ' ') | Should -BeExactly ''
    }
}

Describe 'Complete-SemverChecksCollect' {
    It 'accepts absence of a determined version requirement' {
        { Complete-SemverChecksCollect -ExitCode 0 -LogPath 'semver.log' } |
            Should -Not -Throw
    }

    It 'accepts the documented finding exit' {
        { Complete-SemverChecksCollect -ExitCode 100 -LogPath 'semver.log' } |
            Should -Not -Throw
    }

    It 'throws on a tool error' {
        { Complete-SemverChecksCollect -ExitCode 101 -LogPath 'semver.log' } |
            Should -Throw '*exit 101*'
    }
}

Describe 'Invoke-ReleaseReport' {
    It 'runs SemVer checks only for the explicit report targets' {
        $outDir = Join-Path $TestDrive 'collect'
        $script:calls = [System.Collections.Generic.List[object]]::new()
        $cargo = {
            param([string[]] $Argument)
            $script:calls.Add(@($Argument))
            if ($Argument -contains 'report') {
                $index = [array]::IndexOf($Argument, '--out-dir')
                Write-TestReport -Path (Join-Path $Argument[$index + 1] 'report.json') -Package @(
                    Get-TestPackage -Name 'events' -Status 'needs-increment' `
                        -Changed @(@{ path = 'src/lib.rs' })
                    Get-TestPackage -Name 'folo_utils' -Status 'needs-increment' `
                        -Changed @(@{ path = 'src/lib.rs' })
                )
            } else {
                $global:LASTEXITCODE = 0
                'semver output'
            }
        }

        Invoke-ReleaseReport -OutDir $outDir -Base 'abc' -Cargo $cargo

        $script:calls.Count | Should -Be 2
        $script:calls[1] | Should -Contain 'events'
        $script:calls[1] | Should -Not -Contain 'folo_utils'
        Get-Content -LiteralPath (Join-Path $outDir 'semver-checks.log') -Raw |
            Should -Match 'semver output'
    }

    It 'writes a log and skips cargo-semver-checks when the target set is empty' {
        $outDir = Join-Path $TestDrive 'empty-collect'
        $script:calls = [System.Collections.Generic.List[object]]::new()
        $cargo = {
            param([string[]] $Argument)
            $script:calls.Add(@($Argument))
            $index = [array]::IndexOf($Argument, '--out-dir')
            Write-TestReport -Path (Join-Path $Argument[$index + 1] 'report.json') -Package @(
                Get-TestPackage -Name 'events'
            )
        }

        Invoke-ReleaseReport -OutDir $outDir -Cargo $cargo

        $script:calls.Count | Should -Be 1
        Test-Path -LiteralPath (Join-Path $outDir 'semver-checks.log') | Should -BeTrue
    }
}

Describe 'Invoke-SemverCheck' {
    It 'turns package names into repeated cargo -p arguments' {
        $script:argument = $null
        Invoke-SemverCheck -Package 'events nm' -Cargo {
            param([string[]] $Argument)
            $script:argument = $Argument
        }
        $script:argument | Should -Be @(
            'semver-checks', '--all-features', '-p', 'events', '-p', 'nm'
        )
    }

    It 'does not invoke cargo for an empty package set' {
        $script:called = $false
        Invoke-SemverCheck -Package '' -Cargo { $script:called = $true }
        $script:called | Should -BeFalse
    }
}

Describe 'Invoke-ApplyReleasePlan' {
    It 'passes an existing plan to cargo-release-plan apply' {
        $path = Join-Path $TestDrive 'plan.json'
        '{}' | Set-Content -LiteralPath $path -Encoding utf8
        $script:argument = $null
        Invoke-ApplyReleasePlan -PlanPath $path -Cargo {
            param([string[]] $Argument)
            $script:argument = $Argument
        }
        $script:argument | Should -Contain 'apply'
        $script:argument | Should -Contain $path
    }

    It 'rejects a missing plan' {
        { Invoke-ApplyReleasePlan -PlanPath (Join-Path $TestDrive 'missing.json') } |
            Should -Throw '*not found*'
    }
}

Describe 'Invoke-ValidateVersions' {
    It 'emits released= when the report selects nothing, then runs check' {
        $output = Join-Path $TestDrive 'github-output'
        New-Item -ItemType File -Path $output | Out-Null
        $script:calls = [System.Collections.Generic.List[object]]::new()
        $cargo = {
            param([string[]] $Argument)
            $script:calls.Add(@($Argument))
            if ($Argument -contains 'report') {
                $index = [array]::IndexOf($Argument, '--out-dir')
                Write-TestReport -Path (Join-Path $Argument[$index + 1] 'report.json') -Package @(
                    Get-TestPackage -Name 'events'
                )
            }
        }
        Invoke-ValidateVersions -GitHubOutputPath $output -Base 'abc' -Cargo $cargo
        @(Get-Content -LiteralPath $output) | Should -Be @('released=')
        $script:calls.Count | Should -Be 2
        $script:calls[1] | Should -Contain 'check'
    }

    It 'removes its temporary report directory even when check fails' {
        $output = Join-Path $TestDrive 'failing-github-output'
        New-Item -ItemType File -Path $output | Out-Null
        $script:outDir = $null
        $cargo = {
            param([string[]] $Argument)
            if ($Argument -contains 'report') {
                $index = [array]::IndexOf($Argument, '--out-dir')
                $script:outDir = $Argument[$index + 1]
                Write-TestReport -Path (Join-Path $script:outDir 'report.json') -Package @(
                    Get-TestPackage -Name 'events'
                )
                return
            }
            throw 'cargo-release-plan check found packages needing an increment.'
        }

        { Invoke-ValidateVersions -GitHubOutputPath $output -Base 'abc' -Cargo $cargo } |
            Should -Throw '*needing an increment*'
        $script:outDir | Should -Not -BeNullOrEmpty
        Test-Path -LiteralPath $script:outDir | Should -BeFalse
    }
}

Describe 'Get-ReleasePlanAnalysisBatch' {
    It 'puts dependencies before dependents and combines dependency cycles' {
        $path = Join-Path $TestDrive 'graph.json'
        Write-TestReport -Path $path -Package @(
            Get-TestPackage -Name 'app' -Dependencies @(@{ name = 'middle' })
            Get-TestPackage -Name 'middle' -Dependencies @(@{ name = 'core' })
            Get-TestPackage -Name 'core' -Dependencies @(@{ name = 'middle' })
            Get-TestPackage -Name 'independent'
        )

        $batch = @(Get-ReleasePlanAnalysisBatch -ReportPath $path)

        $cycle = $batch | Where-Object { ($_.packages -join ', ') -eq 'core, middle' }
        $app = $batch | Where-Object { ($_.packages -join ', ') -eq 'app' }
        $cycle.cyclic | Should -BeTrue
        $app.cyclic | Should -BeFalse
        $cycle.order | Should -BeLessThan $app.order
        @($batch.packages) | Sort-Object |
            Should -Be @('app', 'core', 'independent', 'middle')
    }

    It 'keeps a prefix-named dependency out of its dependent''s batch' {
        $path = Join-Path $TestDrive 'prefix.json'
        Write-TestReport -Path $path -Package @(
            Get-TestPackage -Name 'nm' -Dependencies @(@{ name = 'nm_impl' })
            Get-TestPackage -Name 'nm_impl'
        )

        $batch = @(Get-ReleasePlanAnalysisBatch -ReportPath $path)

        $batch.Count | Should -Be 2
        @($batch | Where-Object { $_.cyclic }).Count | Should -Be 0
        $leaf = $batch | Where-Object { ($_.packages -join ', ') -eq 'nm_impl' }
        $dependent = $batch | Where-Object { ($_.packages -join ', ') -eq 'nm' }
        $leaf.order | Should -BeLessThan $dependent.order
    }

    It 'emits the documented JSON field names for the skill working file' {
        $path = Join-Path $TestDrive 'contract.json'
        Write-TestReport -Path $path -Package @(
            Get-TestPackage -Name 'events'
        )

        $json = Get-ReleasePlanAnalysisBatch -ReportPath $path |
            ConvertTo-Json -Depth 3 -AsArray |
            ConvertFrom-Json

        @($json).Count | Should -Be 1
        @($json[0].PSObject.Properties.Name) | Should -Be @('order', 'packages', 'cyclic')
        $json[0].order | Should -Be 1
        @($json[0].packages) | Should -Be @('events')
        $json[0].cyclic | Should -BeFalse
    }
}

Describe 'Assert-IncrementPackagePublished' {
    BeforeAll {
        function Write-TestExpandedPlan {
            param(
                [Parameter(Mandatory)][string] $Path,
                [Parameter(Mandatory)][string[]] $Name
            )

            [ordered]@{
                schema_version = 1
                increments     = @($Name | ForEach-Object { [ordered]@{ name = $_; version = '1.0.1' } })
            } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $Path -Encoding utf8
        }
    }

    It 'checks every package the expansion reached, including group members' {
        $expandedPath = Join-Path $TestDrive 'publish-expanded.json'
        Write-TestExpandedPlan -Path $expandedPath -Name @('nm', 'nm_impl')
        $script:queried = [System.Collections.Generic.List[string]]::new()

        Assert-IncrementPackagePublished -ExpandedPath $expandedPath -GetPublishStatus {
            param([string] $Name)
            $script:queried.Add($Name)
            'Published'
        }

        $script:queried | Should -Be @('nm', 'nm_impl')
    }

    It 'fails when an expanded package was never published' {
        $expandedPath = Join-Path $TestDrive 'new-expanded.json'
        Write-TestExpandedPlan -Path $expandedPath -Name @('events')

        {
            Assert-IncrementPackagePublished -ExpandedPath $expandedPath `
                -GetPublishStatus { 'NeverPublished' }
        } | Should -Throw '*never-published package: events.*First-publish it manually*'
    }

    It 'names every never-published package in the plural' {
        $expandedPath = Join-Path $TestDrive 'plural-expanded.json'
        Write-TestExpandedPlan -Path $expandedPath -Name @('events', 'nm')

        {
            Assert-IncrementPackagePublished -ExpandedPath $expandedPath `
                -GetPublishStatus { 'NeverPublished' }
        } | Should -Throw '*never-published packages: events, nm.*First-publish them manually*'
    }

    It 'fails closed on an expanded plan with an unsupported schema revision' {
        $expandedPath = Join-Path $TestDrive 'bad-schema-expanded.json'
        [ordered]@{
            schema_version = 99
            increments     = @()
        } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $expandedPath -Encoding utf8

        {
            Assert-IncrementPackagePublished -ExpandedPath $expandedPath `
                -GetPublishStatus { 'Published' }
        } | Should -Throw '*schema_version*'
    }

    It 'fails closed on an expanded increment without a name' {
        $expandedPath = Join-Path $TestDrive 'nameless-expanded.json'
        [ordered]@{
            schema_version = 1
            increments     = @([ordered]@{ version = '1.0.1' })
        } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $expandedPath -Encoding utf8

        {
            Assert-IncrementPackagePublished -ExpandedPath $expandedPath `
                -GetPublishStatus { 'Published' }
        } | Should -Throw '*without a name*'
    }
}

Describe 'New-ReleasePlanFile' {
    It 'translates semantic change levels into mechanical Cargo levels' {
        $reportPath = Join-Path $TestDrive 'levels-report.json'
        $decisionPath = Join-Path $TestDrive 'levels-decision.json'
        $planPath = Join-Path $TestDrive 'levels-plan.json'
        Write-TestReport -Path $reportPath -Package @(
            Get-TestPackage -Name 'events' -DeclaredVersion '0.7.0' -AnchorVersion '0.7.0'
            Get-TestPackage -Name 'many_cpus' -DeclaredVersion '2.4.0' -AnchorVersion '2.4.0'
            Get-TestPackage -Name 'nm' -DeclaredVersion '1.0.0' -AnchorVersion '1.0.0'
        )
        Write-TestDecision -Path $decisionPath -Change @(
            @{ name = 'events'; level = 'breaking' }
            @{ name = 'many_cpus'; level = 'breaking' }
            @{ name = 'nm'; level = 'nonbreaking' }
        )

        New-ReleasePlanFile -ReportPath $reportPath -DecisionPath $decisionPath `
            -PlanPath $planPath
        $plan = Get-Content -LiteralPath $planPath -Raw | ConvertFrom-Json

        ($plan.increments | Where-Object name -EQ 'events').level | Should -Be 'minor'
        ($plan.increments | Where-Object name -EQ 'many_cpus').level | Should -Be 'major'
        ($plan.increments | Where-Object name -EQ 'nm').level | Should -Be 'minor'
    }

    It 'realigns a drifted group whose decided level was already covered' {
        # The decision is skipped as already sufficient, so nothing else would name the group.
        # Leaving it unnamed would leave the members disagreeing and the check permanently red.
        $reportPath = Join-Path $TestDrive 'covered-report.json'
        $decisionPath = Join-Path $TestDrive 'covered-decision.json'
        $planPath = Join-Path $TestDrive 'covered-plan.json'
        Write-TestReport -Path $reportPath -Package @(
            Get-TestPackage -Name 'nm' -Group 'nm' -Status 'pending-release' `
                -Changed @(@{ source = 'file' }) -DeclaredVersion '1.1.0' -AnchorVersion '1.0.0'
            Get-TestPackage -Name 'nm_impl' -Group 'nm' -DeclaredVersion '1.0.0' `
                -AnchorVersion '1.0.0'
        ) -Group @{
            nm = @{ members = @('nm', 'nm_impl'); consistent = $false; version = '1.1.0' }
        }
        Write-TestDecision -Path $decisionPath -Change @(
            @{ name = 'nm'; level = 'patch' }
        )

        New-ReleasePlanFile -ReportPath $reportPath -DecisionPath $decisionPath `
            -PlanPath $planPath
        $plan = Get-Content -LiteralPath $planPath -Raw | ConvertFrom-Json

        @($plan.increments).Count | Should -Be 1
        $plan.increments[0].name | Should -Be 'nm'
        $plan.increments[0].version | Should -Be '1.1.0'
        $plan.increments[0].PSObject.Properties.Name | Should -Not -Contain 'level'
    }

    It 'realigns a drifted group that no decision names at all' {
        # Aligning is not a release decision: the members only have to agree, and raising the
        # highest one would publish every member for no substantive change.
        $reportPath = Join-Path $TestDrive 'untouched-report.json'
        $decisionPath = Join-Path $TestDrive 'untouched-decision.json'
        $planPath = Join-Path $TestDrive 'untouched-plan.json'
        Write-TestReport -Path $reportPath -Package @(
            Get-TestPackage -Name 'nm' -Group 'nm' -DeclaredVersion '1.0.0' -AnchorVersion '1.0.0'
            Get-TestPackage -Name 'nm_impl' -Group 'nm' -DeclaredVersion '1.1.0' `
                -AnchorVersion '1.1.0'
        ) -Group @{
            nm = @{ members = @('nm', 'nm_impl'); consistent = $false; version = '1.1.0' }
        }
        Write-TestDecision -Path $decisionPath -Change @()

        New-ReleasePlanFile -ReportPath $reportPath -DecisionPath $decisionPath `
            -PlanPath $planPath
        $plan = Get-Content -LiteralPath $planPath -Raw | ConvertFrom-Json

        @($plan.increments).Count | Should -Be 1
        $plan.increments[0].name | Should -Be 'nm'
        $plan.increments[0].version | Should -Be '1.1.0'
    }

    It 'leaves a drifted group to the change level that already names a member' {
        # A level entry realigns the group on its own. Adding an exact version beside it would
        # give one group two decision kinds, which the tool rejects.
        $reportPath = Join-Path $TestDrive 'named-report.json'
        $decisionPath = Join-Path $TestDrive 'named-decision.json'
        $planPath = Join-Path $TestDrive 'named-plan.json'
        Write-TestReport -Path $reportPath -Package @(
            Get-TestPackage -Name 'nm' -Group 'nm' -Status 'needs-increment' `
                -Changed @(@{ source = 'file' }) -DeclaredVersion '1.0.0' -AnchorVersion '1.0.0'
            Get-TestPackage -Name 'nm_impl' -Group 'nm' -DeclaredVersion '1.1.0' `
                -AnchorVersion '1.1.0'
        ) -Group @{
            nm = @{ members = @('nm', 'nm_impl'); consistent = $false; version = '1.1.0' }
        }
        Write-TestDecision -Path $decisionPath -Change @(
            @{ name = 'nm'; level = 'patch' }
        )

        New-ReleasePlanFile -ReportPath $reportPath -DecisionPath $decisionPath `
            -PlanPath $planPath
        $plan = Get-Content -LiteralPath $planPath -Raw | ConvertFrom-Json

        @($plan.increments).Count | Should -Be 1
        $plan.increments[0].level | Should -Be 'patch'
    }

    It 'leaves a consistent group alone' {
        $reportPath = Join-Path $TestDrive 'consistent-report.json'
        $decisionPath = Join-Path $TestDrive 'consistent-decision.json'
        $planPath = Join-Path $TestDrive 'consistent-plan.json'
        Write-TestReport -Path $reportPath -Package @(
            Get-TestPackage -Name 'nm' -Group 'nm' -DeclaredVersion '1.0.0' -AnchorVersion '1.0.0'
        ) -Group @{
            nm = @{ members = @('nm'); consistent = $true; version = '1.0.0' }
        }
        Write-TestDecision -Path $decisionPath -Change @()

        New-ReleasePlanFile -ReportPath $reportPath -DecisionPath $decisionPath `
            -PlanPath $planPath
        $plan = Get-Content -LiteralPath $planPath -Raw | ConvertFrom-Json

        @($plan.increments).Count | Should -Be 0
    }

    It 'rejects a change level the skill does not decide' {
        $reportPath = Join-Path $TestDrive 'unsupported-level-report.json'
        $decisionPath = Join-Path $TestDrive 'unsupported-level-decision.json'
        Write-TestReport -Path $reportPath -Package @(
            Get-TestPackage -Name 'nm' -Group 'nm' -DeclaredVersion '1.0.0' -AnchorVersion '1.0.0'
        ) -Group @{
            nm = @{ members = @('nm'); consistent = $false; version = '1.0.0' }
        }
        Write-TestDecision -Path $decisionPath -Change @(
            @{ name = 'nm'; level = 'align' }
        )

        {
            New-ReleasePlanFile -ReportPath $reportPath -DecisionPath $decisionPath `
                -PlanPath (Join-Path $TestDrive 'unsupported-level-plan.json')
        } | Should -Throw "*unsupported level 'align'*"
    }

    It 'rejects a prerelease version rather than deriving a level from it' {
        # A prerelease orders below the release it precedes, so no component comparison can
        # express "drop the suffix"; a derived level would silently overshoot.
        $reportPath = Join-Path $TestDrive 'prerelease-report.json'
        $decisionPath = Join-Path $TestDrive 'prerelease-decision.json'
        Write-TestReport -Path $reportPath -Package @(
            Get-TestPackage -Name 'events' -DeclaredVersion '1.1.0-alpha' -AnchorVersion '1.0.0'
        )
        Write-TestDecision -Path $decisionPath -Change @(
            @{ name = 'events'; level = 'nonbreaking' }
        )

        {
            New-ReleasePlanFile -ReportPath $reportPath -DecisionPath $decisionPath `
                -PlanPath (Join-Path $TestDrive 'prerelease-plan.json')
        } | Should -Throw '*prerelease version*'
    }

    It 'keeps a compatible change to a 0.y package on its patch component' {
        # Cargo treats 0.7.15 as compatible with 0.7.14, so an addition must not consume the
        # minor component and strand consumers on the old requirement.
        $reportPath = Join-Path $TestDrive 'zero-minor-report.json'
        $decisionPath = Join-Path $TestDrive 'zero-minor-decision.json'
        $planPath = Join-Path $TestDrive 'zero-minor-plan.json'
        Write-TestReport -Path $reportPath -Package @(
            Get-TestPackage -Name 'events' -DeclaredVersion '0.7.14' -AnchorVersion '0.7.14'
        )
        Write-TestDecision -Path $decisionPath -Change @(
            @{ name = 'events'; level = 'nonbreaking' }
        )

        New-ReleasePlanFile -ReportPath $reportPath -DecisionPath $decisionPath `
            -PlanPath $planPath
        $plan = Get-Content -LiteralPath $planPath -Raw | ConvertFrom-Json

        $plan.increments[0].level | Should -Be 'patch'
    }

    It 'advances the minor component for a breaking change to a 0.y package' {
        $reportPath = Join-Path $TestDrive 'zero-break-report.json'
        $decisionPath = Join-Path $TestDrive 'zero-break-decision.json'
        $planPath = Join-Path $TestDrive 'zero-break-plan.json'
        Write-TestReport -Path $reportPath -Package @(
            Get-TestPackage -Name 'events' -DeclaredVersion '0.7.14' -AnchorVersion '0.7.14'
        )
        Write-TestDecision -Path $decisionPath -Change @(
            @{ name = 'events'; level = 'breaking' }
        )

        New-ReleasePlanFile -ReportPath $reportPath -DecisionPath $decisionPath `
            -PlanPath $planPath
        $plan = Get-Content -LiteralPath $planPath -Raw | ConvertFrom-Json

        $plan.increments[0].level | Should -Be 'minor'
    }

    It 'confines every change level to the patch component of a 0.0.z package' {
        # No 0.0.z release is compatible with another, so there is no component left for a
        # breaking change to advance beyond the one a patch already advances.
        $reportPath = Join-Path $TestDrive 'zero-zero-report.json'
        $decisionPath = Join-Path $TestDrive 'zero-zero-decision.json'
        $planPath = Join-Path $TestDrive 'zero-zero-plan.json'
        Write-TestReport -Path $reportPath -Package @(
            Get-TestPackage -Name 'events' -DeclaredVersion '0.0.5' -AnchorVersion '0.0.5'
            Get-TestPackage -Name 'nm' -DeclaredVersion '0.0.5' -AnchorVersion '0.0.5'
        )
        Write-TestDecision -Path $decisionPath -Change @(
            @{ name = 'events'; level = 'breaking' }
            @{ name = 'nm'; level = 'nonbreaking' }
        )

        New-ReleasePlanFile -ReportPath $reportPath -DecisionPath $decisionPath `
            -PlanPath $planPath
        $plan = Get-Content -LiteralPath $planPath -Raw | ConvertFrom-Json

        ($plan.increments | Where-Object name -EQ 'events').level | Should -Be 'patch'
        ($plan.increments | Where-Object name -EQ 'nm').level | Should -Be 'patch'
    }

    It 'does not lower or repeat an already sufficient pending increment' {
        $reportPath = Join-Path $TestDrive 'pending-report.json'
        $decisionPath = Join-Path $TestDrive 'pending-decision.json'
        $planPath = Join-Path $TestDrive 'pending-plan.json'
        Write-TestReport -Path $reportPath -Package @(
            Get-TestPackage -Name 'events' -Status 'pending-release' `
                -DeclaredVersion '0.8.0' -AnchorVersion '0.7.0'
        )
        Write-TestDecision -Path $decisionPath -Change @(
            @{ name = 'events'; level = 'breaking' }
        )

        New-ReleasePlanFile -ReportPath $reportPath -DecisionPath $decisionPath `
            -PlanPath $planPath
        $plan = Get-Content -LiteralPath $planPath -Raw | ConvertFrom-Json

        @($plan.increments).Count | Should -Be 0
    }

    It 'emits a group member decision for cargo-release-plan to merge at apply time' {
        $reportPath = Join-Path $TestDrive 'group-report.json'
        $decisionPath = Join-Path $TestDrive 'group-decision.json'
        $planPath = Join-Path $TestDrive 'group-plan.json'
        Write-TestReport -Path $reportPath -Package @(
            Get-TestPackage -Name 'nm' -Group 'nm' `
                -DeclaredVersion '1.2.0' -AnchorVersion '1.0.0'
            Get-TestPackage -Name 'nm_impl' -Group 'nm' `
                -DeclaredVersion '1.2.0' -AnchorVersion '1.2.0'
        ) -Group @{
            nm = @{ members = @('nm', 'nm_impl'); consistent = $true; version = '1.2.0' }
        }
        Write-TestDecision -Path $decisionPath -Change @(
            @{ name = 'nm'; level = 'nonbreaking' }
            @{ name = 'nm_impl'; level = 'patch' }
        )

        New-ReleasePlanFile -ReportPath $reportPath -DecisionPath $decisionPath `
            -PlanPath $planPath
        $plan = Get-Content -LiteralPath $planPath -Raw | ConvertFrom-Json

        @($plan.increments).Count | Should -Be 1
        $plan.increments[0].name | Should -Be 'nm_impl'
        $plan.increments[0].level | Should -Be 'patch'
    }

    It 'rejects a decision entry that carries an exact version' {
        $reportPath = Join-Path $TestDrive 'invalid-report.json'
        $decisionPath = Join-Path $TestDrive 'invalid-decision.json'
        Write-TestReport -Path $reportPath -Package @(
            Get-TestPackage -Name 'events'
        )
        '{"schema_version":1,"changes":[{"name":"events","level":"patch","version":"9.0.0"}]}' |
            Set-Content -LiteralPath $decisionPath -Encoding utf8

        {
            New-ReleasePlanFile -ReportPath $reportPath -DecisionPath $decisionPath `
                -PlanPath (Join-Path $TestDrive 'invalid-plan.json')
        } | Should -Throw '*only name and level*'
    }

    It 'rejects a Cargo increment level in place of a semantic change level' {
        $reportPath = Join-Path $TestDrive 'cargo-level-report.json'
        $decisionPath = Join-Path $TestDrive 'cargo-level-decision.json'
        Write-TestReport -Path $reportPath -Package @(
            Get-TestPackage -Name 'events'
        )
        # `minor` is a Cargo increment level; the decision file speaks semantic change levels.
        '{"schema_version":1,"changes":[{"name":"events","level":"minor"}]}' |
            Set-Content -LiteralPath $decisionPath -Encoding utf8

        {
            New-ReleasePlanFile -ReportPath $reportPath -DecisionPath $decisionPath `
                -PlanPath (Join-Path $TestDrive 'cargo-level-plan.json')
        } | Should -Throw "*unsupported level 'minor'*"
    }

    It 'rejects a semantic change level that differs only by case' {
        $reportPath = Join-Path $TestDrive 'case-report.json'
        $decisionPath = Join-Path $TestDrive 'case-decision.json'
        Write-TestReport -Path $reportPath -Package @(
            Get-TestPackage -Name 'events'
        )
        '{"schema_version":1,"changes":[{"name":"events","level":"Breaking"}]}' |
            Set-Content -LiteralPath $decisionPath -Encoding utf8

        {
            New-ReleasePlanFile -ReportPath $reportPath -DecisionPath $decisionPath `
                -PlanPath (Join-Path $TestDrive 'case-plan.json')
        } | Should -Throw "*unsupported level 'Breaking'*"
    }
}
