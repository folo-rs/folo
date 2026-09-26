#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Protects Standard validation's change domains, whole-candidate Git comparisons and explicit
# no-work results. Native Git fixtures cover deletions/renames without GitHub or a Rust setup.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'ValidationPlan.psm1') -Force
    $script:allDomains = @('analyzer', 'bench-history', 'book', 'build', 'release', 'scheduled', 'setup', 'utility')
    function ConvertTo-PlanJson($Plan) { ConvertTo-Json -InputObject $Plan -Compress }
}

Describe 'Non-Cargo change domains' {
    It 'does not select tooling for ordinary Rust or documentation changes' {
        $plan = Get-ValidationPlan -ChangedPath @('packages/events_once/src/lib.rs', 'README.md',
            'docs/testing.md', '.github/workflows/design.md', 'Cargo.lock')
        $plan.workflows | Should -BeFalse
        $plan.script_analysis | Should -BeFalse
        $plan.bicep | Should -BeFalse
        $plan.script_domains | Should -BeNullOrEmpty
        ConvertTo-PlanJson $plan | Should -Match '"script_domains":\[\]'
    }

    It 'selects the owner and its consumers for <Path>' -ForEach @(
        @{ Path = 'scripts/book/BookSite.psm1'; Domains = @('book'); Analysis = $true; Workflows = $false },
        @{ Path = 'scripts/book/BookSite.Tests.ps1'; Domains = @('book'); Analysis = $true; Workflows = $false },
        @{ Path = 'scripts/bench-history/fixtures/result.json'; Domains = @('bench-history'); Analysis = $false; Workflows = $false },
        @{ Path = 'scripts/build/Miri.psm1'; Domains = @('build', 'scheduled'); Analysis = $true; Workflows = $false },
        @{ Path = 'scripts/build/CargoExecutable.psm1'; Domains = @('build', 'release', 'scheduled'); Analysis = $true; Workflows = $false },
        @{ Path = 'scripts/release/ReleasePlan.psm1'; Domains = @('release'); Analysis = $true; Workflows = $false },
        @{ Path = 'PSScriptAnalyzerSettings.psd1'; Domains = @('analyzer'); Analysis = $true; Workflows = $false },
        @{ Path = '.github/workflows/release.yml'; Domains = @('build', 'release', 'scheduled'); Analysis = $false; Workflows = $true },
        @{ Path = '.github/actionlint.yaml'; Domains = @('build', 'scheduled'); Analysis = $false; Workflows = $true },
        @{ Path = '.github/actions/setup-workflow-lint/action.yml'; Domains = @('build', 'scheduled'); Analysis = $false; Workflows = $true },
        @{ Path = 'justfiles/just_bench_history.just'; Domains = @('bench-history'); Analysis = $false; Workflows = $false },
        @{ Path = 'infra/azure-bench-history-prod/main.bicep'; Domains = @('bench-history', 'build', 'scheduled'); Analysis = $false; Workflows = $false },
        @{ Path = 'infra/azure-bench-history-prod/deploy.ps1'; Domains = @('bench-history'); Analysis = $true; Workflows = $false },
        @{ Path = 'infra/azure-bench-history-test/deploy.ps1'; Domains = @('bench-history'); Analysis = $true; Workflows = $false },
        @{ Path = 'infra/azure-bench-history-test/main.bicepparam'; Domains = @('bench-history', 'build', 'scheduled'); Analysis = $false; Workflows = $false },
        @{ Path = 'packages/cargo-bench-history/src/azure_bundle/main.bicep'; Domains = @('bench-history', 'build', 'scheduled'); Analysis = $false; Workflows = $false },
        @{ Path = 'packages/cargo-bench-history/src/azure_bundle/deploy.ps1'; Domains = @('bench-history'); Analysis = $true; Workflows = $false },
        @{ Path = 'packages/cargo-bench-history/src/azure_bundle/AzureDeployment.psm1'; Domains = @('bench-history'); Analysis = $true; Workflows = $false },
        @{ Path = 'packages/cargo-bench-history/tests/fixtures/setup-azure.ps1'; Domains = @('bench-history'); Analysis = $true; Workflows = $false },
        @{ Path = '.github/actions/bench-history-setup/action.yml'; Domains = @('bench-history', 'build', 'scheduled'); Analysis = $false; Workflows = $true },
        @{ Path = '.github/workflows/bench-history.yml'; Domains = @('bench-history', 'build', 'scheduled'); Analysis = $false; Workflows = $true },
        @{ Path = '.github/workflows/pr-bench-history.yml'; Domains = @('bench-history', 'build', 'scheduled'); Analysis = $false; Workflows = $true },
        @{ Path = '.github/workflows/bench-history-backfill.yml'; Domains = @('bench-history', 'build', 'scheduled'); Analysis = $false; Workflows = $true },
        @{ Path = 'justfiles/just_release.just'; Domains = @('release'); Analysis = $false; Workflows = $false },
        @{ Path = 'justfiles/just_quality.just'; Domains = @('bench-history', 'build', 'scheduled'); Analysis = $true; Workflows = $true },
        @{ Path = '.cargo/mutants.toml'; Domains = @('build', 'scheduled'); Analysis = $false; Workflows = $false },
        @{ Path = 'Cargo.toml'; Domains = @('scheduled'); Analysis = $false; Workflows = $false },
        @{ Path = 'packages/cpulist/Cargo.toml'; Domains = @('scheduled'); Analysis = $false; Workflows = $false },
        @{ Path = '.github/skills/scheduled-triage/SKILL.md'; Domains = @('scheduled'); Analysis = $false; Workflows = $false },
        @{ Path = '.github/skills/scheduled-intake/SKILL.md'; Domains = @('scheduled'); Analysis = $false; Workflows = $false },
        @{ Path = '.github/skills/scheduled-repair/SKILL.md'; Domains = @('scheduled'); Analysis = $false; Workflows = $false },
        @{ Path = '.github/prompts/setup-scheduled-remediation.prompt.md'; Domains = @('scheduled'); Analysis = $false; Workflows = $false },
        @{ Path = 'docs/scheduled-validation.md'; Domains = @('scheduled'); Analysis = $false; Workflows = $false }
    ) {
        $plan = Get-ValidationPlan -ChangedPath @($Path)
        $plan.script_domains | Should -Be $Domains
        $plan.script_analysis | Should -Be $Analysis
        $plan.workflows | Should -Be $Workflows
    }

    It 'selects every tooling check for shared input <_>' -ForEach @(
        'scripts/build/ValidationPlan.psm1', 'scripts/build/ValidationPlan.Tests.ps1',
        'scripts/build/RequiredChecks.psm1', 'scripts/build/RequiredChecks.Tests.ps1',
        'scripts/setup/install-actionlint.ps1', 'scripts/setup/install-shellcheck.ps1',
        'scripts/setup/RustToolchain.psm1', 'scripts/utility/Retry.psm1',
        '.github/actions/setup-environment/action.yml', 'justfile',
        'justfiles/just_setup.just', 'justfiles/just_testing.just', 'constants.env',
        'rust-toolchain.toml', '.gitattributes', '.gitconfig'
    ) {
        $plan = Get-ValidationPlan -ChangedPath @($_)
        $plan.workflows | Should -BeTrue
        $plan.script_analysis | Should -BeTrue
        $plan.bicep | Should -BeTrue
        $plan.script_domains | Should -Be $allDomains
    }

    It 'does not silently exclude unfamiliar scripts or recipes' -ForEach @(
        'scripts/new-domain/New.Tests.ps1', 'scripts/standalone.ps1', 'justfiles/new.just'
    ) {
        (Get-ValidationPlan -ChangedPath @($_)).script_domains | Should -Be $allDomains
    }

    It 'selects offline compilation for maintained Bicep input <_>' -ForEach @(
        'infra/azure-bench-history-test/main.bicep',
        'infra/azure-bench-history-test/main.bicepparam',
        'packages/cargo-bench-history/src/azure_bundle/container-bootstrap.bicep',
        'bicepconfig.json', 'scripts/build/Bicep.psm1', 'scripts/build/Bicep.Tests.ps1',
        'justfiles/just_quality.just'
    ) {
        (Get-ValidationPlan -ChangedPath @($_)).bicep | Should -BeTrue
    }

    It 'keeps the pairing recipe in the release test domain' {
        $plan = Get-ValidationPlan -ChangedPath @('justfiles/just_benchmark_action.just')
        $plan.script_domains | Should -Be @('release')
        $plan.bicep | Should -BeFalse
    }

    It 'unions and deduplicates domains' {
        $plan = Get-ValidationPlan -ChangedPath @('scripts/book/BookSite.psm1',
            'scripts/book/BookSite.Tests.ps1', 'scripts/build/Miri.psm1')
        $plan.script_domains | Should -Be @('book', 'build', 'scheduled')
    }

    It 'runs all tooling for <_> on main without needing a comparison' -ForEach @(
        'push', 'schedule', 'workflow_dispatch'
    ) {
        $plan = Get-ValidationWorkflowPlan -EventName $_ -EventData @{} -Ref 'refs/heads/main' -Repository 'folo-rs/folo'
        $plan.workflows | Should -BeTrue
        $plan.script_analysis | Should -BeTrue
        $plan.bicep | Should -BeTrue
        $plan.script_domains | Should -Be $allDomains
    }

    It 'rejects full-scope <_> runs outside main' -ForEach @('push', 'schedule', 'workflow_dispatch') {
        { Get-ValidationWorkflowPlan -EventName $_ -EventData @{} -Ref 'refs/heads/feature' -Repository 'folo-rs/folo' } | Should -Throw
    }

    It 'rejects events that do not belong to Standard validation' -ForEach @('merge_group', 'workflow_run') {
        { Get-ValidationWorkflowPlan -EventName $_ -EventData @{} -Ref 'refs/heads/main' -Repository 'folo-rs/folo' } | Should -Throw
    }
}

Describe 'Caller integration selection' {
    It 'selects the canary for its own input <_>' -ForEach @(
        '.github/fixtures/bench-history-caller/Cargo.lock',
        '.github/fixtures/bench-history-caller/packages/workflow_canary/benches/synthetic.rs',
        '.github/workflows/benchmark-action-canary.yml', '.github/workflows/standard-validation.yml',
        '.github/workflows/deep-validation.yml', 'scripts/bench-history/CallerFixture.psm1',
        'scripts/bench-history/Assert-BackfillCanary.ps1', '.github/actions/bench-history-setup/action.yml',
        '.cargo/config.toml', 'delta.toml', 'constants.env', 'justfiles/just_quality.just',
        '.github/actions/setup-environment/action.yml', 'scripts/build/Delta.psm1', '.gitignore'
    ) {
        $plan = ConvertTo-PlanJson (Get-ValidationPlan -ChangedPath @($_) -CanaryTrusted $true)
        $selection = Get-ValidationCanarySelection -PlanJson $plan -AffectedPackageJson '[]'
        $selection.check_fixture | Should -BeTrue
        $selection.run_hosted | Should -BeTrue
        @(Get-ValidationScriptDomain -PlanJson $plan -AffectedPackageJson '[]') | Should -Contain 'bench-history'
    }

    It 'uses transitive consumer impact for <_> rather than enumerating private partitions' -ForEach @(
        'cargo-bench-history', 'cargo-bench-history-github', 'cargo-bench-history-faker'
    ) {
        $plan = ConvertTo-PlanJson (Get-ValidationPlan -ChangedPath @('packages/cbh_storage/src/lib.rs') -CanaryTrusted $true)
        $selection = Get-ValidationCanarySelection -PlanJson $plan `
            -AffectedPackageJson (ConvertTo-Json -InputObject @('cbh_storage', $_))
        $selection.run_hosted | Should -BeTrue
    }

    It 'does not select Azure for unrelated documentation, packages or workflows' {
        $plan = ConvertTo-PlanJson (Get-ValidationPlan -ChangedPath @(
                'docs/testing.md', 'packages/events_once/src/lib.rs', '.github/workflows/book.yml'
            ) -CanaryTrusted $true)
        $selection = Get-ValidationCanarySelection -PlanJson $plan -AffectedPackageJson '["events_once"]'
        $selection.check_fixture | Should -BeFalse
        $selection.run_hosted | Should -BeFalse
    }

    It 'keeps the credential-free preflight but excludes hosted work for untrusted events' {
        $plan = ConvertTo-PlanJson (Get-ValidationPlan -ChangedPath @() -CanaryTrusted $false)
        $selection = Get-ValidationCanarySelection -PlanJson $plan -AffectedPackageJson '["cargo-bench-history"]'
        $selection.check_fixture | Should -BeTrue
        $selection.run_hosted | Should -BeFalse
    }

    It 'runs full canary scope for main <_> including reusable scheduled callers' -ForEach @(
        'push', 'schedule', 'workflow_dispatch'
    ) {
        $plan = Get-ValidationWorkflowPlan -EventName $_ -EventData @{} -Ref 'refs/heads/main' -Repository 'folo-rs/folo'
        (Get-ValidationCanarySelection -PlanJson (ConvertTo-PlanJson $plan) -AffectedPackageJson '[]').run_hosted |
            Should -BeTrue
        $fork = Get-ValidationWorkflowPlan -EventName $_ -EventData @{} -Ref 'refs/heads/main' -Repository 'fork/folo'
        (Get-ValidationCanarySelection -PlanJson (ConvertTo-PlanJson $fork) -AffectedPackageJson '[]').run_hosted |
            Should -BeFalse
    }

    It 'rejects absent or malformed canary selection fields' -ForEach @(
        'benchmark_canary', 'benchmark_canary_trusted'
    ) {
        $plan = Get-ValidationPlan -ChangedPath @()
        $plan.Remove($_)
        { Read-ValidationPlan -Json (ConvertTo-PlanJson $plan) } | Should -Throw
        $plan[$_] = 'false'
        { Read-ValidationPlan -Json (ConvertTo-PlanJson $plan) } | Should -Throw
    }
}

Describe 'Cargo helper integration selection' {
    It 'adds release tests for affected helper <_>' -ForEach @(
        'cargo-release-plan', 'crp_impl', 'release-target-check', 'release-binaries'
    ) {
        $plan = ConvertTo-PlanJson (Get-ValidationPlan -ChangedPath @('scripts/book/BookSite.psm1'))
        $packages = ConvertTo-Json -InputObject @($_) -Compress
        @(Get-ValidationScriptDomain -PlanJson $plan -AffectedPackageJson $packages) |
            Should -Be @('book', 'release')
    }

    It 'does not select scripts for unrelated Cargo dependency impact' {
        $plan = ConvertTo-PlanJson (Get-ValidationPlan -ChangedPath @('Cargo.lock'))
        @(Get-ValidationScriptDomain -PlanJson $plan -AffectedPackageJson '["events_once"]') |
            Should -BeNullOrEmpty
    }

    It 'preserves path-selected scripts when Cargo selects nothing' {
        $plan = ConvertTo-PlanJson (Get-ValidationPlan -ChangedPath @('scripts/book/BookSite.psm1'))
        @(Get-ValidationScriptDomain -PlanJson $plan -AffectedPackageJson '[]') | Should -Be @('book')
    }

    It 'rejects missing or malformed package outputs' -ForEach @('', 'null', '{}', '"crate"', '[1]') {
        $plan = ConvertTo-PlanJson (Get-ValidationPlan -ChangedPath @())
        { Get-ValidationScriptDomain -PlanJson $plan -AffectedPackageJson $_ } | Should -Throw
    }

    It 'rejects missing or malformed path plans' -ForEach @(
        '', 'null', '{}', '{"workflows":false,"script_analysis":false}',
        '{"workflows":"false","script_analysis":false,"script_domains":[]}',
        '{"workflows":false,"script_analysis":false,"script_domains":"book"}',
        '{"workflows":false,"script_analysis":false,"script_domains":["unknown"]}'
    ) {
        { Read-ValidationPlan -Json $_ } | Should -Throw
    }
}

Describe 'Release binary smoke selection' {
    It 'selects the native smoke for release adapter and shared setup inputs' -ForEach @(
        '.github/workflows/release.yml', '.github/workflows/standard-validation.yml', 'justfiles/just_release.just',
        'scripts/release/ReleaseBinaries.psm1',
        'scripts/setup/ReleaseArchiveTools.psm1', 'scripts/build/RequiredChecks.psm1',
        '.cargo/config.toml'
    ) {
        $plan = ConvertTo-PlanJson (Get-ValidationPlan -ChangedPath @($_))
        Test-ReleaseBinarySmokeSelected -PlanJson $plan -AffectedPackageJson '[]' | Should -BeTrue
    }

    It 'selects helper dependency impact without unrelated Cargo impact' {
        $plan = ConvertTo-PlanJson (Get-ValidationPlan -ChangedPath @('Cargo.lock'))
        Test-ReleaseBinarySmokeSelected -PlanJson $plan -AffectedPackageJson '["release-binaries"]' | Should -BeTrue
        Test-ReleaseBinarySmokeSelected -PlanJson $plan -AffectedPackageJson '["crp_impl"]' | Should -BeTrue
        Test-ReleaseBinarySmokeSelected -PlanJson $plan -AffectedPackageJson '["events_once"]' | Should -BeFalse
    }

    It 'rejects malformed helper dependency impact' -ForEach @('null', '{}', '[1]') {
        $plan = ConvertTo-PlanJson (Get-ValidationPlan -ChangedPath @())
        { Test-ReleaseBinarySmokeSelected -PlanJson $plan -AffectedPackageJson $_ } | Should -Throw
    }
}

Describe 'Explicit Pester scope' {
    It 'retains the full local default' {
        Get-ScriptTestPath -Root $TestDrive | Should -Be $TestDrive
    }

    It 'resolves and deduplicates selected suites and rejects unknown or empty domains' {
        $path = Join-Path $TestDrive 'book'
        $null = New-Item -ItemType Directory -Path $path -Force
        Set-Content -LiteralPath (Join-Path $path 'Book.Tests.ps1') -Value '# fixture'
        @(Get-ScriptTestPath -Domains 'book book' -Root $TestDrive) | Should -Be @($path)
        { Get-ScriptTestPath -Domains 'unknown' -Root $TestDrive } | Should -Throw
        { Get-ScriptTestPath -Domains 'release' -Root $TestDrive } | Should -Throw
        $null = New-Item -ItemType Directory -Path (Join-Path $TestDrive 'release') -Force
        { Get-ScriptTestPath -Domains 'release' -Root $TestDrive } | Should -Throw
    }

    It 'covers every current test directory with the full selection' {
        $root = Join-Path $PSScriptRoot '..'
        $actual = @(Get-ChildItem -LiteralPath $root -Directory | Where-Object {
                @(Get-ChildItem -LiteralPath $_.FullName -Filter '*.Tests.ps1' -Recurse -File).Count -gt 0
            } | ForEach-Object { $_.Name } | Sort-Object)
        $actual | Should -Be $allDomains
        @(Get-ScriptTestPath -Domains ($allDomains -join ' ')).Count | Should -Be $allDomains.Count
    }
}

Describe 'Complete Git change sets' {
    BeforeEach {
        $repo = Join-Path $TestDrive ([guid]::NewGuid().ToString('N'))
        $null = New-Item -ItemType Directory -Path $repo
        Push-Location $repo
        git init --quiet --initial-branch=main
        git config user.name 'Validation fixture'
        git config user.email 'fixture@example.invalid'
        git config commit.gpgsign false
        $null = New-Item -ItemType Directory -Path 'scripts/book', 'scripts/build', 'scripts/setup'
        Set-Content -LiteralPath 'scripts/book/old name.ps1' -Value '# book fixture'
        Set-Content -LiteralPath 'README.md' -Value 'fixture'
        git add .
        git commit --quiet -m 'Fixture baseline'
        $base = git rev-parse HEAD
        function Save-FixtureCommit {
            git add --all
            git commit --quiet -m 'Fixture change'
            return git rev-parse HEAD
        }
        function Get-FixturePlan([string] $Base, [string] $Head) {
            $eventData = @{ pull_request = @{ base = @{ sha = $Base }; head = @{
                        sha = $Head; repo = @{ full_name = 'folo-rs/folo' }
                    } } }
            Get-ValidationWorkflowPlan -EventName pull_request -EventData $eventData -Ref 'refs/pull/1/merge' -Repository 'folo-rs/folo'
        }
    }
    AfterEach { Pop-Location }

    It 'includes early PR commits even when the latest commit only touches documentation' {
        Add-Content -LiteralPath 'scripts/book/old name.ps1' -Value '# changed'
        $null = Save-FixtureCommit
        Add-Content -LiteralPath 'README.md' -Value 'later'
        $head = Save-FixtureCommit
        (Get-FixturePlan $base $head).script_domains | Should -Be @('book')
    }

    It 'excludes changes made only on the advanced PR base branch' {
        Add-Content -LiteralPath 'scripts/book/old name.ps1' -Value '# changed'
        $head = Save-FixtureCommit
        git switch --quiet -c advanced-base $base
        Set-Content -LiteralPath 'scripts/setup/unrelated.ps1' -Value '# base-only'
        $newBase = Save-FixtureCommit
        (Get-FixturePlan $newBase $head).script_domains | Should -Be @('book')
    }

    It 'includes both domains of a rename across the entire pull request' {
        Move-Item -LiteralPath 'scripts/book/old name.ps1' -Destination 'scripts/build/new name.ps1'
        $null = Save-FixtureCommit
        Add-Content -LiteralPath 'README.md' -Value 'later change'
        $head = Save-FixtureCommit
        (Get-FixturePlan $base $head).script_domains | Should -Be @('book', 'build', 'scheduled')
    }

    It 'retains deletions and emits an explicit empty plan for identical commits' {
        Remove-Item -LiteralPath 'scripts/book/old name.ps1'
        $head = Save-FixtureCommit
        (Get-FixturePlan $base $head).script_domains | Should -Be @('book')
        $empty = Get-FixturePlan $head $head
        $empty.script_domains | Should -BeNullOrEmpty
        $empty.script_analysis | Should -BeFalse
        $empty.workflows | Should -BeFalse
    }

    It 'fails for missing or unavailable event revisions' {
        { Get-ValidationWorkflowPlan -EventName pull_request -EventData @{} -Ref 'refs/pull/1/merge' -Repository 'folo-rs/folo' } | Should -Throw
        { Get-FixturePlan $base ('f' * 40) } | Should -Throw
    }

    It 'distinguishes same-repository and fork PR credentials from the event' -ForEach @(
        @{ HeadRepository = 'folo-rs/folo'; Trusted = $true }
        @{ HeadRepository = 'fork/folo'; Trusted = $false }
    ) {
        $eventData = @{ pull_request = @{ base = @{ sha = $base }; head = @{
                    sha = $base; repo = @{ full_name = $HeadRepository }
                } } }
        $plan = Get-ValidationWorkflowPlan -EventName pull_request -EventData $eventData `
            -Ref 'refs/pull/1/merge' -Repository 'folo-rs/folo'
        $plan.benchmark_canary_trusted | Should -Be $Trusted
    }
}
