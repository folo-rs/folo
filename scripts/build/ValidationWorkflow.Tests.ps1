#requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Checks relationships between workflow outputs, validation fan-ins, dependencies and job definitions
# without invoking GitHub jobs or freezing workflow settings as test literals.
# Ref: .github/workflows/implementation.md#merge-blocking-result.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    $root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
    $script:standard = Get-Content -LiteralPath (Join-Path $root '.github/workflows/standard-validation.yml') -Raw
    $script:deep = Get-Content -LiteralPath (Join-Path $root '.github/workflows/deep-validation.yml') -Raw
    $script:queue = Get-Content -LiteralPath (Join-Path $root '.github/workflows/merge-queue-validation.yml') -Raw
    $script:canary = Get-Content -LiteralPath (Join-Path $root '.github/workflows/benchmark-action-canary.yml') -Raw
    $script:benchmarkWorkflows = @(
        foreach ($name in @('bench-history', 'pr-bench-history', 'bench-history-backfill', 'benchmark-action-canary')) {
            Get-Content -LiteralPath (Join-Path $root ".github/workflows/$name.yml") -Raw
        }
    )
    Import-Module (Join-Path $PSScriptRoot 'RequiredChecks.psm1') -Force

    function Get-WorkflowJob([string] $Workflow, [string] $Name) {
        $pattern = '(?ms)^  ' + [regex]::Escape($Name) + ':\r?\n(?<body>.*?)(?=^  [a-z][a-z0-9-]*:\r?$|\z)'
        $match = [regex]::Match($Workflow, $pattern)
        $match.Success | Should -BeTrue
        return $match.Groups['body'].Value
    }

    function Get-WorkflowJobName([string] $Workflow) {
        $jobs = [regex]::Match($Workflow, '(?ms)^jobs:\r?\n(?<body>.*?)(?=^\S|\z)')
        $jobs.Success | Should -BeTrue
        return [regex]::Matches($jobs.Groups['body'].Value, '(?m)^  ([a-z][a-z0-9-]*):\r?$') |
            ForEach-Object { $_.Groups[1].Value }
    }

    function Get-WorkflowJobDependency([string] $Job) {
        if ($Job -notmatch '(?m)^    needs:') { return @() }

        $scalar = [regex]::Match($Job, '(?m)^    needs: (?<job>[a-z][a-z0-9-]*)\r?$')
        if ($scalar.Success) { return $scalar.Groups['job'].Value }

        $inline = [regex]::Match($Job, '(?m)^    needs: \[(?<jobs>[^\]]+)\]')
        if ($inline.Success) {
            return $inline.Groups['jobs'].Value -split ',' | ForEach-Object { $_.Trim() }
        }

        $block = [regex]::Match($Job, '(?m)^    needs:\r?\n(?<jobs>(?:      - [^\r\n]+\r?\n)+)')
        $block.Success | Should -BeTrue
        return [regex]::Matches($block.Groups['jobs'].Value, '(?m)^      - ([^\r\n]+)') |
            ForEach-Object { $_.Groups[1].Value.Trim() }
    }

    function Get-WorkflowEvent([string] $Workflow) {
        $events = [regex]::Match($Workflow, '(?ms)^on:\r?\n(?<body>.*?)(?=^\S|\z)')
        $events.Success | Should -BeTrue
        return [regex]::Matches($events.Groups['body'].Value, '(?m)^  ([a-z_]+):') |
            ForEach-Object { $_.Groups[1].Value }
    }

    function Get-MustSucceedJob([string] $FanIn) {
        return [regex]::Match($FanIn, '(?m)^\s+MUST_SUCCEED_JOBS: ([^\r\n]+)').Groups[1].Value -split '\s+'
    }

    function Assert-WorkflowIdentityHandoff([string] $Workflow) {
        foreach ($name in @(Get-WorkflowJobName $Workflow)) {
            $job = Get-WorkflowJob $Workflow $name
            if ($job -notmatch '(?m)^    uses: .*cargo-bench-history-action/') { continue }

            # Only job-output bindings can be lost through Azure login masking.
            # Repository variables need no producer; storage ordering is a separate concern.
            $inputs = [regex]::Matches($job,
                '(?m)^      azure-(?:client|tenant)-id: \$\{\{ needs\.(?<job>[a-z][a-z0-9-]*)\.outputs\.(?<output>[a-z][a-z0-9-]*) \}\}')
            foreach ($inputReference in $inputs) {
                $producerName = $inputReference.Groups['job'].Value
                $outputName = $inputReference.Groups['output'].Value
                @(Get-WorkflowJobDependency $job) | Should -Contain $producerName
                $producer = Get-WorkflowJob $Workflow $producerName
                $producer | Should -Match ('(?m)^      ' + [regex]::Escape($outputName) + ': ')
                $producer | Should -Not -Match '(?m)^\s+(?:- )?uses: azure/login@'
            }
        }
    }
}

Describe 'Workflow dependency extraction' {
    It 'accepts equivalent inline and block lists' {
        $inline = @(Get-WorkflowJobDependency "    needs: [plan, checks]`n")
        $block = @(Get-WorkflowJobDependency "    needs:`n      - plan`n      - checks`n")
        $inline | Should -Be @('plan', 'checks')
        $block | Should -Be $inline
    }

    It 'accepts a scalar dependency or no dependency' {
        @(Get-WorkflowJobDependency "    needs: plan`n") | Should -Be @('plan')
        @(Get-WorkflowJobDependency "    runs-on: ubuntu-latest`n") | Should -BeNullOrEmpty
    }
}

Describe 'Validation job references' {
    It 'resolves every declared prerequisite within its workflow' {
        foreach ($workflow in (@($standard, $deep, $queue) + $benchmarkWorkflows)) {
            $jobNames = @(Get-WorkflowJobName $workflow)
            foreach ($name in $jobNames) {
                $job = Get-WorkflowJob $workflow $name
                foreach ($dependency in @(Get-WorkflowJobDependency $job)) {
                    $jobNames | Should -Contain $dependency
                    $dependency | Should -Not -Be $name
                }
            }
        }
    }
}

Describe 'Shared environment preparation order' {
    It 'completes toolchain preparation before the build cache reads compiler identities' {
        $action = Get-Content -LiteralPath (Join-Path $root '.github/actions/setup-environment/action.yml') -Raw
        $steps = @([regex]::Matches($action, '(?ms)^    - .*?(?=^    - |\z)') |
            ForEach-Object { $_.Value })
        $preparation = @($steps | Where-Object { $_ -match '(?m)^\s+Install-RustToolchain\s*$' })
        $cache = @($steps | Where-Object { $_ -match '(?m)^\s+uses: Swatinem/rust-cache@' })
        $preparation.Count | Should -Be 1
        $cache.Count | Should -Be 1
        [array]::IndexOf($steps, $preparation[0]) |
            Should -BeLessThan ([array]::IndexOf($steps, $cache[0]))
    }
}

Describe 'Benchmark caller identity handoff' {
    BeforeAll {
        # Synthetic workflows exercise the relationship check independently of repository settings.
        $script:identityOutputWorkflow = @'
jobs:
  identities:
    runs-on: ubuntu-latest
    outputs:
      client-id: ${{ steps.ids.outputs.client }}
    steps:
      - uses: example/read-identifiers@v1
  storage:
    runs-on: ubuntu-latest
    steps:
      - uses: azure/login@v3
  collect:
    needs: identities
    uses: example/cargo-bench-history-action/.github/workflows/history.yml@v1
    with:
      azure-client-id: ${{ needs.identities.outputs.client-id }}
      azure-tenant-id: ${{ vars.TENANT_ID }}
'@
    }

    It 'accepts repository variables without prerequisite jobs' {
        $workflow = @'
jobs:
  collect:
    uses: example/cargo-bench-history-action/.github/workflows/history.yml@v1
    with:
      azure-client-id: ${{ vars.CLIENT_ID }}
      azure-tenant-id: ${{ vars.TENANT_ID }}
'@
        { Assert-WorkflowIdentityHandoff $workflow } | Should -Not -Throw
    }

    It 'accepts an unmasked producer without depending on unrelated login jobs' {
        { Assert-WorkflowIdentityHandoff $identityOutputWorkflow } | Should -Not -Throw
    }

    It 'rejects <Case> for a job-output identity binding' -ForEach @(
        @{ Case = 'an undeclared dependency'; Before = '    needs: identities'; After = '' }
        @{ Case = 'an undeclared output'; Before = '      client-id:'; After = '      other-id:' }
        @{ Case = 'a masking producer'; Before = 'example/read-identifiers@v1'; After = 'azure/login@v3' }
    ) {
        $workflow = $identityOutputWorkflow.Replace($Before, $After)
        { Assert-WorkflowIdentityHandoff $workflow } | Should -Throw
    }

    It 'validates actual cross-job identity bindings without imposing an execution graph' {
        foreach ($workflow in $benchmarkWorkflows) {
            Assert-WorkflowIdentityHandoff $workflow
        }
    }
}

Describe 'Deep validation dependency relationships' {
    It 'reuses the standard workflow behind the main-only plan gate' {
        $caller = Get-WorkflowJob $deep 'standard'
        $calledPath = [regex]::Match($caller, '(?m)^    uses: ([^\r\n]+)').Groups[1].Value
        $called = Get-Content -LiteralPath (Join-Path $root $calledPath) -Raw
        $called | Should -Be $standard
        @(Get-WorkflowEvent $called) | Should -Contain 'workflow_call'
        @(Get-WorkflowJobDependency $caller) | Should -Be @(Get-WorkflowJobDependency (Get-WorkflowJob $deep 'checks'))
    }

    It 'reports failures from every planning and check job without stale dependencies' {
        $jobNames = @(Get-WorkflowJobName $deep | Where-Object { $_ -ne 'report' })
        $report = Get-WorkflowJob $deep 'report'
        $dependencies = @(Get-WorkflowJobDependency $report)
        foreach ($job in $jobNames) { $dependencies | Should -Contain $job }
        foreach ($job in $dependencies) { $jobNames | Should -Contain $job }
    }
}

Describe 'Standard validation dependency relationships' {
    It 'requires unconditional jobs and keeps every dependency in the workflow' {
        foreach ($workflow in @($standard, $queue)) {
            $jobNames = @(Get-WorkflowJobName $workflow)
            $fanIn = Get-WorkflowJob $workflow 'required-checks'
            $dependencies = @(Get-WorkflowJobDependency $fanIn)
            $mustSucceed = @(Get-MustSucceedJob $fanIn)
            foreach ($job in $mustSucceed) { $dependencies | Should -Contain $job }
            foreach ($job in $dependencies) {
                $jobNames | Should -Contain $job
                if ((Get-WorkflowJob $workflow $job) -notmatch '(?m)^    if:') {
                    $mustSucceed | Should -Contain $job
                }
            }
        }
    }

    Describe 'Required reusable canary relationships' {
        It 'keeps backfill calls in distinct configuration-keyed concurrency groups' {
            $configs = @(foreach ($name in @('backfill', 'rolling-backfill', 'no-eligible-backfill')) {
                    $job = Get-WorkflowJob $canary $name
                    $config = [regex]::Match($job, '(?m)^      config: ([^\r\n]+)').Groups[1].Value
                    $config | Should -Not -BeNullOrEmpty
                    Test-Path -LiteralPath (Join-Path $root ".github/fixtures/bench-history-caller/$config") |
                        Should -BeTrue
                    $config
                })
            @($configs | Sort-Object -Unique).Count | Should -Be $configs.Count
        }

        It 'has one reusable entry point selected by Standard validation rather than duplicate event runs' {
            @(Get-WorkflowEvent $canary) | Should -Be @('workflow_call')
            $caller = Get-WorkflowJob $standard 'benchmark-canary'
            $calledPath = [regex]::Match($caller, '(?m)^    uses: ([^\r\n]+)').Groups[1].Value
            (Get-Content -LiteralPath (Join-Path $root $calledPath) -Raw) | Should -Be $canary
            @(Get-WorkflowJobDependency $caller) | Should -Contain 'prepare'
            $output = [regex]::Match($caller, 'needs\.prepare\.outputs\.([a-z_]+)').Groups[1].Value
            $output | Should -Not -BeNullOrEmpty
            (Get-WorkflowJob $standard 'prepare') | Should -Match ('(?m)^      ' + [regex]::Escape($output) + ': ')
            @(Get-WorkflowJobName $queue) | Should -Not -Contain 'benchmark-canary'
        }

        It 'propagates every contract job failure, cancellation, unexpected skip or absence' {
            $fanIn = Get-WorkflowJob $canary 'result'
            $dependencies = @(Get-WorkflowJobDependency $fanIn)
            $mustSucceed = @(Get-MustSucceedJob $fanIn)
            $checks = @(Get-WorkflowJobName $canary | Where-Object { $_ -ne 'result' })
            @($dependencies | Sort-Object) | Should -Be @($checks | Sort-Object)
            @($mustSucceed | Sort-Object) | Should -Be @($checks | Sort-Object)
            $results = @{}
            foreach ($name in $checks) { $results[$name] = @{ result = 'success' } }
            { Assert-RequiredCheck -NeedsJson (ConvertTo-Json $results) -MustSucceedJob $mustSucceed } |
                Should -Not -Throw
            foreach ($name in $checks) {
                foreach ($result in @('failure', 'cancelled', 'skipped', 'unknown')) {
                    $results[$name].result = $result
                    { Assert-RequiredCheck -NeedsJson (ConvertTo-Json $results) -MustSucceedJob $mustSucceed } |
                        Should -Throw
                }
                $results.Remove($name)
                { Assert-RequiredCheck -NeedsJson (ConvertTo-Json $results) -MustSucceedJob $mustSucceed } |
                    Should -Throw
                $results[$name] = @{ result = 'success' }
            }
        }
    }

    It 'classifies every blocking job and reports every substantive job directly' {
        $jobNames = @(Get-WorkflowJobName $standard)
        $blocking = @($jobNames | Where-Object { $_ -notin @('required-checks', 'alert', 'coverage-notify') })
        $reported = @($jobNames | Where-Object { $_ -notin @('required-checks', 'alert') })
        $fanIn = @(Get-WorkflowJobDependency (Get-WorkflowJob $standard 'required-checks'))
        $alert = @(Get-WorkflowJobDependency (Get-WorkflowJob $standard 'alert'))
        @($fanIn | Sort-Object) | Should -Be @($blocking | Sort-Object)
        @($alert | Sort-Object) | Should -Be @($reported | Sort-Object)
    }
}

Describe 'Merge queue validation relationships' {
    It 'reports the same required check without overlapping Standard validation events' {
        $standardFanIn = Get-WorkflowJob $standard 'required-checks'
        $queueFanIn = Get-WorkflowJob $queue 'required-checks'
        $namePattern = '(?m)^    name: ([^\r\n]+)'
        [regex]::Match($queueFanIn, $namePattern).Groups[1].Value |
            Should -Be ([regex]::Match($standardFanIn, $namePattern).Groups[1].Value)
        $standardEvents = @(Get-WorkflowEvent $standard)
        foreach ($eventName in @(Get-WorkflowEvent $queue)) {
            $standardEvents | Should -Not -Contain $eventName
        }
    }

    It 'keeps the standard compile-platform coverage' {
        $pattern = '(?m)^        platform: \[([^\]]+)\]'
        $standardPlatforms = [regex]::Match((Get-WorkflowJob $standard 'clippy-dev-docs'), $pattern).Groups[1].Value
        $queuePlatforms = [regex]::Match((Get-WorkflowJob $queue 'clippy-dev'), $pattern).Groups[1].Value
        $queuePlatforms | Should -Not -BeNullOrEmpty
        $queuePlatforms | Should -Be $standardPlatforms
    }

    It 'requires every queue check and rejects unsuccessful or missing results' {
        $fanIn = Get-WorkflowJob $queue 'required-checks'
        $dependencies = @(Get-WorkflowJobDependency $fanIn)
        $mustSucceed = @(Get-MustSucceedJob $fanIn)
        $checks = @(Get-WorkflowJobName $queue | Where-Object { $_ -ne 'required-checks' })
        @($dependencies | Sort-Object) | Should -Be @($checks | Sort-Object)
        @($mustSucceed | Sort-Object) | Should -Be @($checks | Sort-Object)
        $results = @{}
        foreach ($name in $checks) { $results[$name] = @{ result = 'success' } }
        { Assert-RequiredCheck -NeedsJson (ConvertTo-Json $results) -MustSucceedJob $mustSucceed } |
            Should -Not -Throw
        foreach ($name in $checks) {
            foreach ($result in @('failure', 'cancelled', 'skipped', 'unknown')) {
                $results[$name].result = $result
                { Assert-RequiredCheck -NeedsJson (ConvertTo-Json $results) -MustSucceedJob $mustSucceed } |
                    Should -Throw
            }
            $results.Remove($name)
            { Assert-RequiredCheck -NeedsJson (ConvertTo-Json $results) -MustSucceedJob $mustSucceed } |
                Should -Throw
            $results[$name] = @{ result = 'success' }
        }
    }
}
