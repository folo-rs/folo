#requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Checks relationships between workflow outputs, validation fan-ins, dependencies and job definitions
# without invoking GitHub jobs or freezing workflow settings as test literals.
# Ref: .github/workflows/implementation.md#merge-blocking-result.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    # Only this conjunction proves event HEAD is already on Folo's release branch.
    $script:mainHistoryBinding = '${{ github.event_name != ''pull_request'' && github.ref == ''refs/heads/main'' && github.sha || '''' }}'

    $root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
    $script:standard = Get-Content -LiteralPath (Join-Path $root '.github/workflows/standard-validation.yml') -Raw
    $script:deep = Get-Content -LiteralPath (Join-Path $root '.github/workflows/deep-validation.yml') -Raw
    $script:queue = Get-Content -LiteralPath (Join-Path $root '.github/workflows/merge-queue-validation.yml') -Raw
    $script:release = Get-Content -LiteralPath (Join-Path $root '.github/workflows/release.yml') -Raw
    $script:canary = Get-Content -LiteralPath (Join-Path $root '.github/workflows/benchmark-action-canary.yml') -Raw
    $script:publicApiPublisher = Get-Content -LiteralPath (
        Join-Path $root '.github/workflows/public-api-diff-publish.yml') -Raw
    $script:benchmarkWorkflows = @(
        foreach ($name in @('bench-history', 'pr-bench-history', 'bench-history-backfill', 'benchmark-action-canary')) {
            Get-Content -LiteralPath (Join-Path $root ".github/workflows/$name.yml") -Raw
        }
    )
    Import-Module (Join-Path $PSScriptRoot '..\build\RequiredChecks.psm1') -Force

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

    function Get-WorkflowPrerequisite([string] $Workflow, [string] $Name) {
        $seen = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        $pending = [System.Collections.Generic.Stack[string]]::new()
        $pending.Push($Name)
        while ($pending.Count -gt 0) {
            foreach ($dependency in @(Get-WorkflowJobDependency (Get-WorkflowJob $Workflow $pending.Pop()))) {
                if ($seen.Add($dependency)) {
                    $dependency
                    $pending.Push($dependency)
                }
            }
        }
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

    function Get-ReleaseSmokeStep([string] $Workflow) {
        $job = Get-WorkflowJob $Workflow 'clippy-dev-docs'
        return @([regex]::Matches($job, '(?ms)^      - .*?(?=^      - |\z)') |
            ForEach-Object { $_.Value } |
            Where-Object { $_ -match '(?m)^\s+just release-binary-smoke\s*$' })
    }

    function Assert-ReleaseSmokeRelationship([string] $Workflow) {
        $steps = @(Get-ReleaseSmokeStep $Workflow)
        $steps.Count | Should -Be 1
        $condition = [regex]::Match($steps[0], '(?m)^\s+if:\s*(?<condition>[^\r\n]+)').Groups['condition'].Value
        $condition | Should -Match '!\s*cancelled\(\)'
        $condition | Should -Match 'steps\.setup\.outcome\s*==\s*''success'''
        $condition | Should -Match 'needs\.prepare\.outputs\.release_binary_smoke\s*==\s*''true'''
        @(Get-WorkflowJobDependency (Get-WorkflowJob $Workflow 'clippy-dev-docs')) | Should -Contain 'prepare'
        @(Get-WorkflowJobDependency (Get-WorkflowJob $Workflow 'required-checks')) | Should -Contain 'clippy-dev-docs'
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

    Describe 'Release smoke execution relationship' {
        It 'connects the selected smoke command to its hosting job and required fan-in' {
            Assert-ReleaseSmokeRelationship $standard
        }

        It 'rejects a removed or disabled smoke step' {
            $step = @(Get-ReleaseSmokeStep $standard)[0]
            { Assert-ReleaseSmokeRelationship ($standard.Replace($step, '')) } | Should -Throw
            $disabled = [regex]::Replace($step, '(?m)^(\s+)if:[^\r\n]+', '$1if: false')
            { Assert-ReleaseSmokeRelationship ($standard.Replace($step, $disabled)) } | Should -Throw
        }
    }

    It 'accepts a scalar dependency or no dependency' {
        @(Get-WorkflowJobDependency "    needs: plan`n") | Should -Be @('plan')
        @(Get-WorkflowJobDependency "    runs-on: ubuntu-latest`n") | Should -BeNullOrEmpty
    }
}

Describe 'Release assessment history and target handoff' {
    BeforeAll {
        function Assert-KnownMainHistory([string] $Workflow) {
            $job = Get-WorkflowJob $Workflow 'validate-versions'
            $history = @([regex]::Matches($job,
                '(?m)^      release-history:\s*(?<value>[^\r\n]+)'))
            $history.Count | Should -Be 1
            $history[0].Groups['value'].Value | Should -BeExactly $script:mainHistoryBinding
            $job | Should -Not -Match '(?m)^      (base|merge-target):'
        }

        function Assert-QueueTargetHandoff([string] $Workflow) {
            $job = Get-WorkflowJob $Workflow 'validate-versions'
            $steps = @([regex]::Matches($job, '(?ms)^      - .*?(?=^      - |\z)') |
                ForEach-Object { $_.Value })
            $install = @($steps | Where-Object { $_ -match 'uses: folo-rs/cargo-release-plan-action@' })
            $check = @($steps | Where-Object { $_ -match 'Invoke-ReleaseValidation' })
            $install.Count | Should -Be 1
            $check.Count | Should -Be 1
            $id = [regex]::Match($install[0], '(?m)^\s+id:\s*(?<id>[^\s]+)').Groups['id'].Value
            $id | Should -Not -BeNullOrEmpty
            $install[0] | Should -Match '(?m)^\s+command: version\r?$'
            $check[0] | Should -Match ([regex]::Escape("CRP_EXE: `${{ steps.$id.outputs.executable }}"))
            $check[0] | Should -Match 'QUEUE_TARGET: \$\{\{ github\.event\.merge_group\.base_sha \}\}'
            $check[0] | Should -Match 'Invoke-ReleaseValidation -MergeTarget \$env:QUEUE_TARGET -VersionReadinessOnly -Tool'
            $check[0] | Should -Match '& \$env:CRP_EXE @Argument'
            $job | Should -Not -Match 'RELEASE_PLAN_HISTORY:|RELEASE_PLAN_BASE:|--release-history|cargo run|verify-semver-checks| semver-checks|validate-binstall'
            $job | Should -Match '(?m)^\s+fetch-depth:\s*0\s*$'
        }
    }

    It 'separates guarded Standard history from the queue target' {
        Assert-KnownMainHistory $standard
        Assert-QueueTargetHandoff $queue
    }

    It 'rejects a lost queue target or a target relabeled as history' {
        foreach ($name in @('RELEASE_PLAN_HISTORY', 'RELEASE_PLAN_BASE', 'UNRELATED_INPUT')) {
            $changed = $queue.Replace('QUEUE_TARGET:', "$name`:")
            { Assert-QueueTargetHandoff $changed } | Should -Throw
        }
    }

    It 'rejects event HEAD as history without both non-PR and main-ref guards' {
        foreach ($binding in @(
            '${{ github.sha }}',
            '${{ github.event_name != ''pull_request'' && github.sha || '''' }}',
            '${{ github.ref == ''refs/heads/main'' && github.sha || '''' }}'
        )) {
            $changed = $standard.Replace($script:mainHistoryBinding, $binding)
            { Assert-KnownMainHistory $changed } | Should -Throw
        }
    }

    It 'rejects lost installed-tool linkage or accidentally widened queue validation' {
        { Assert-QueueTargetHandoff ($queue.Replace('steps.tool.outputs.executable', 'steps.unrelated.outputs.executable')) } |
            Should -Throw
        { Assert-QueueTargetHandoff ($queue.Replace('-VersionReadinessOnly', '')) } | Should -Throw
    }
}

Describe 'Shared release integration' {
    It 'pins every shared release entry point to the same immutable action revision' {
        $references = @([regex]::Matches(($standard, $queue, $release -join "`n"),
                'uses: folo-rs/cargo-release-plan-action(?:/\.github/workflows/[^@\r\n]+)?@(?<revision>[^\s]+)') |
            ForEach-Object { $_.Groups['revision'].Value })
        $references.Count | Should -BeGreaterThan 0
        foreach ($revision in $references) { $revision | Should -Match '^[0-9a-f]{40}$' }
        @($references | Sort-Object -Unique).Count | Should -Be 1
    }

    It 'keeps the complete shared check unconditional and read-only within the existing fan-in' {
        $check = Get-WorkflowJob $standard 'validate-versions'
        $check | Should -Match '(?m)^    uses: folo-rs/cargo-release-plan-action/\.github/workflows/check\.yml@'
        $check | Should -Not -Match '(?m)^    (if|needs):'
        $check | Should -Not -Match '(?m)^\s+(id-token|contents|issues): write'
        @(Get-WorkflowJobDependency (Get-WorkflowJob $standard 'required-checks')) |
            Should -Contain 'validate-versions'
        @(Get-MustSucceedJob (Get-WorkflowJob $standard 'required-checks')) |
            Should -Contain 'validate-versions'
    }

    It 'keeps the queue separate from the complete shared check graph' {
        $check = Get-WorkflowJob $queue 'validate-versions'
        $check | Should -Match '-VersionReadinessOnly'
        $check | Should -Not -Match '(?m)^\s+(config|deny-findings):'
        $check | Should -Not -Match '/\.github/workflows/check\.yml@'
    }

    It 'keeps controller installation and explicit recovery-source routing distinct' {
        $publisher = Get-WorkflowJob $release 'publish'
        $publisher | Should -Match 'uses: folo-rs/cargo-release-plan-action/\.github/workflows/release\.yml@'
        $publisher | Should -Match '(?m)^      install-method: path\r?$'
        $publisher | Should -Match '(?m)^      source-path: \.\r?$'
        $publisher | Should -Match 'source: \$\{\{ inputs\.source \|\| '''' \}\}'
        $release | Should -Match '(?m)^      source:\r?$'
        $publisher | Should -Not -Match '(?m)^    (steps|runs-on):'
    }

    It 'makes the exchange/revoke probe and publisher mutually exclusive on the registered caller' {
        $probe = Get-WorkflowJob $release 'verify-publishing-identity'
        $publisher = Get-WorkflowJob $release 'publish'
        $probe | Should -Match '/\.github/workflows/identity-probe\.yml@'
        $probe | Should -Match '&& inputs\.verify-publishing-identity &&'
        $publisher | Should -Match '&& !inputs\.verify-publishing-identity'
        foreach ($job in @($probe, $publisher)) {
            $job | Should -Match 'github\.repository == ''folo-rs/folo'''
            $job | Should -Match 'github\.ref == ''refs/heads/main'''
        }
        $probe | Should -Not -Match '(?m)^\s+(contents|issues): write'
    }

    It 'retains the migration lock without cancelling or replacing queued publisher runs' {
        $lock = [regex]::Match($release, '(?ms)^concurrency:\r?\n(?<body>.*?)(?=^\S|\z)').Groups['body'].Value
        $lock | Should -Match 'group: release-\$\{\{ github\.ref \}\}'
        $lock | Should -Match '(?m)^  cancel-in-progress: false\r?$'
        $lock | Should -Match '(?m)^  queue: max\r?$'
        # release-context uses its workspace-scoped prefix for the nested graph's lock.
        $lock | Should -Not -Match 'group: cargo-release-plan-'
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

Describe 'Public API diff publisher trust boundary' {
    It 'runs from workflow completion with comment-only default-branch authority' {
        @(Get-WorkflowEvent $publicApiPublisher) | Should -Be @('workflow_run')
        $publicApiPublisher | Should -Match '(?m)^  actions: read\r?$'
        $publicApiPublisher | Should -Match '(?m)^  pull-requests: write\r?$'
        $publicApiPublisher | Should -Not -Match '(?m)^  (contents|issues): write\r?$'
    }

    It 'checks out the trusted default branch and never executes producer files' {
        $job = Get-WorkflowJob $publicApiPublisher 'publish'
        $job | Should -Match 'ref: \$\{\{ github\.event\.repository\.default_branch \}\}'
        $job | Should -Not -Match 'github\.event\.workflow_run\.(head_sha|head_branch).*checkout'
        $job | Should -Match 'actions/download-artifact@v8'
        $job | Should -Match 'PublishPublicApiComment\.ps1'
    }

    It 'binds the publisher to the immutable producer run identity' {
        $job = Get-WorkflowJob $publicApiPublisher 'publish'
        $job | Should -Match 'name: public-api-diff-\$\{\{ github\.event\.workflow_run\.id \}\}-\$\{\{ github\.event\.workflow_run\.run_attempt \}\}'
        $job | Should -Match 'run-id: \$\{\{ github\.event\.workflow_run\.id \}\}'
        $job | Should -Match 'PUBLIC_API_HEAD_SHA: \$\{\{ github\.event\.workflow_run\.head_sha \}\}'
        $job | Should -Match 'PUBLIC_API_HEAD_REPOSITORY: \$\{\{ github\.event\.workflow_run\.head_repository\.full_name \}\}'
        $job | Should -Not -Match 'pull_requests\[0\]'
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
        It 'keeps caller lanes and cases in distinct concurrency and storage namespaces' {
            $projects = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
            $configs = @(foreach ($name in @('exercise', 'backfill', 'rolling-backfill', 'no-eligible-backfill')) {
                    $job = Get-WorkflowJob $canary $name
                    $config = [regex]::Match($job, '(?m)^      config: ([^\r\n]+)')
                    $config.Success | Should -BeTrue
                    $paths = [regex]::Matches($config.Groups[1].Value, '\.cargo/\w+\.toml')
                    $paths.Count | Should -Be $(if ($name -in @('exercise', 'backfill')) { 2 } else { 1 })
                    foreach ($path in $paths) {
                        $content = Get-Content -LiteralPath (Join-Path $root ".github/fixtures/bench-history-caller/$($path.Value)") -Raw
                        $project = [regex]::Match($content, '(?m)^id = "([^"]+)"').Groups[1].Value
                        $project | Should -Not -BeNullOrEmpty
                        $projects.Add($project) | Should -BeTrue
                        $path.Value
                    }
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

        It 'keeps macOS collection and backfill out of the selected required call' {
            (Get-WorkflowJob $standard 'benchmark-canary') | Should -Not -Match '(?m)^      macos-only:'
            (Get-WorkflowJob $standard 'benchmark-canary-macos') | Should -Match '(?m)^      macos-only: true\r?$'
            $canary | Should -Match '(?m)^      macos-only:\r?\n(?:        [^\r\n]+\r?\n)*?        default: false\r?$'
            (Get-WorkflowJob $canary 'exercise') | Should -Match '(?m)^      platforms: &canary-platforms \$\{\{ inputs\.macos-only && ''macos-latest'' \|\| ''ubuntu-latest,windows-latest'' \}\}\r?$'
            (Get-WorkflowJob $canary 'backfill') | Should -Match '(?m)^      platforms: \*canary-platforms\r?$'
            foreach ($name in @('rolling-backfill', 'no-eligible-backfill')) {
                (Get-WorkflowJob $canary $name) | Should -Match '(?m)^    if: ''!inputs\.macos-only''\r?$'
            }
        }

        It 'propagates every required contract job failure, cancellation, unexpected skip or absence' {
            $fanIn = Get-WorkflowJob $canary 'result'
            $dependencies = @(Get-WorkflowJobDependency $fanIn)
            $linuxOnly = [regex]::Match($fanIn, "(?m)LINUX_ONLY_JOBS: .*&& '([^']+)'").Groups[1].Value -split ' '
            $common = @(Get-MustSucceedJob $fanIn)
            $mustSucceed = $common + $linuxOnly
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
            foreach ($name in $linuxOnly) { $results[$name].result = 'skipped' }
            { Assert-RequiredCheck -NeedsJson (ConvertTo-Json $results) -MustSucceedJob $common } |
                Should -Not -Throw
        }
    }

    It 'keeps both macOS lanes off the required and main-push alert paths' {
        $jobNames = @(Get-WorkflowJobName $standard)
        $advisory = @('clippy-dev-docs-macos', 'benchmark-canary-macos')
        $blocking = @($jobNames | Where-Object { $_ -notin (@('required-checks', 'alert', 'coverage-notify') + $advisory) })
        $reported = @($blocking + 'coverage-notify')
        $fanIn = @(Get-WorkflowJobDependency (Get-WorkflowJob $standard 'required-checks'))
        $alert = @(Get-WorkflowJobDependency (Get-WorkflowJob $standard 'alert'))
        @($fanIn | Sort-Object) | Should -Be @($blocking | Sort-Object)
        @($alert | Sort-Object) | Should -Be @($reported | Sort-Object)
        foreach ($gate in @('required-checks', 'alert')) {
            $prerequisites = @(Get-WorkflowPrerequisite $standard $gate)
            foreach ($name in $advisory) { $prerequisites | Should -Not -Contain $name }
        }
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
        $queuePlatforms | Should -Be 'ubuntu-latest, windows-latest'
        $queuePlatforms | Should -Be $standardPlatforms
        foreach ($workflow in @(
                @{ Content = $standard; Job = 'clippy-dev-docs' }
                @{ Content = $queue; Job = 'clippy-dev' }
            )) {
            $required = Get-WorkflowJob $workflow.Content $workflow.Job
            $mac = Get-WorkflowJob $workflow.Content "$($workflow.Job)-macos"
            $sharedSteps = [regex]::Match($required, '(?m)^    steps: &([\w-]+)').Groups[1].Value
            $sharedSteps | Should -Not -BeNullOrEmpty
            $mac | Should -Match '(?m)^    runs-on: macos-latest\r?$'
            $mac | Should -Match ('(?m)^    steps: \*' + [regex]::Escape($sharedSteps) + '\r?$')
            $mac | Should -Not -Match 'continue-on-error:'
            @(Get-WorkflowPrerequisite $workflow.Content 'required-checks') |
                Should -Not -Contain "$($workflow.Job)-macos"
        }
    }

    It 'requires every queue check and rejects unsuccessful or missing results' {
        $fanIn = Get-WorkflowJob $queue 'required-checks'
        $dependencies = @(Get-WorkflowJobDependency $fanIn)
        $mustSucceed = @(Get-MustSucceedJob $fanIn)
        $checks = @(Get-WorkflowJobName $queue | Where-Object { $_ -notin @('required-checks', 'clippy-dev-macos') })
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
