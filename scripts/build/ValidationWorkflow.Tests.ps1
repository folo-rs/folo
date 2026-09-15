#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Checks relationships between validation fan-ins, dependencies and job definitions
# without invoking GitHub jobs or freezing workflow settings as test literals.
# Ref: .github/workflows/implementation.md#merge-blocking-result.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    $root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
    $script:standard = Get-Content -LiteralPath (Join-Path $root '.github/workflows/standard-validation.yml') -Raw
    $script:deep = Get-Content -LiteralPath (Join-Path $root '.github/workflows/deep-validation.yml') -Raw

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
        $inline = [regex]::Match($Job, '(?m)^    needs: \[(?<jobs>[^\]]+)\]')
        if ($inline.Success) {
            return $inline.Groups['jobs'].Value -split ',' | ForEach-Object { $_.Trim() }
        }

        $block = [regex]::Match($Job, '(?m)^    needs:\r?\n(?<jobs>(?:      - [^\r\n]+\r?\n)+)')
        $block.Success | Should -BeTrue
        return [regex]::Matches($block.Groups['jobs'].Value, '(?m)^      - ([^\r\n]+)') |
            ForEach-Object { $_.Groups[1].Value.Trim() }
    }
}

Describe 'Workflow dependency extraction' {
    It 'accepts equivalent inline and block lists' {
        $inline = @(Get-WorkflowJobDependency "    needs: [plan, checks]`n")
        $block = @(Get-WorkflowJobDependency "    needs:`n      - plan`n      - checks`n")
        $inline | Should -Be @('plan', 'checks')
        $block | Should -Be $inline
    }
}

Describe 'Deep validation dependency relationships' {
    It 'reports failures from every planning and check job without stale dependencies' {
        $jobNames = @(Get-WorkflowJobName $deep | Where-Object { $_ -ne 'report' })
        $report = Get-WorkflowJob $deep 'report'
        $dependencies = @(Get-WorkflowJobDependency $report)
        foreach ($job in $jobNames) { $dependencies | Should -Contain $job }
        foreach ($job in $dependencies) { $jobNames | Should -Contain $job }
    }
}

Describe 'Standard validation dependency relationships' {
    It 'keeps every must-succeed job in the fan-in and every dependency in the workflow' {
        $jobNames = @(Get-WorkflowJobName $standard)
        $fanIn = Get-WorkflowJob $standard 'required-checks'
        $dependencies = @(Get-WorkflowJobDependency $fanIn)
        $mustSucceed = [regex]::Match($fanIn, '(?m)^\s+MUST_SUCCEED_JOBS: ([^\r\n]+)').Groups[1].Value -split '\s+'
        foreach ($job in $mustSucceed) { $dependencies | Should -Contain $job }
        foreach ($job in $dependencies) { $jobNames | Should -Contain $job }
    }
}
