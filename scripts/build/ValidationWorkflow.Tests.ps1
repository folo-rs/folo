#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Checks relationships between Standard validation's fan-in, dependencies and job definitions
# without invoking GitHub jobs or freezing workflow settings as test literals.
# Ref: .github/workflows/implementation.md#merge-blocking-result.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    $root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
    $script:standard = Get-Content -LiteralPath (Join-Path $root '.github/workflows/standard-validation.yml') -Raw

    function Get-WorkflowJob([string] $Workflow, [string] $Name) {
        $pattern = '(?ms)^  ' + [regex]::Escape($Name) + ':\r?\n(?<body>.*?)(?=^  [a-z][a-z0-9-]*:\r?$|\z)'
        $match = [regex]::Match($Workflow, $pattern)
        $match.Success | Should -BeTrue
        return $match.Groups['body'].Value
    }
}

Describe 'Standard validation dependency relationships' {
    It 'keeps every must-succeed job in the fan-in and every dependency in the workflow' {
        $jobNames = @([regex]::Matches($standard, '(?m)^  ([a-z][a-z0-9-]*):\r?$') |
            ForEach-Object { $_.Groups[1].Value })
        $fanIn = Get-WorkflowJob $standard 'required-checks'
        $dependencyBlock = [regex]::Match($fanIn, '(?m)^    needs:\r?\n(?<jobs>(?:      - [^\r\n]+\r?\n)+)')
        $dependencyBlock.Success | Should -BeTrue
        $dependencies = @([regex]::Matches($dependencyBlock.Groups['jobs'].Value, '(?m)^      - ([^\r\n]+)') |
            ForEach-Object { $_.Groups[1].Value })
        $mustSucceed = [regex]::Match($fanIn, '(?m)^\s+MUST_SUCCEED_JOBS: ([^\r\n]+)').Groups[1].Value -split '\s+'
        foreach ($job in $mustSucceed) { $dependencies | Should -Contain $job }
        foreach ($job in $dependencies) { $jobNames | Should -Contain $job }
    }
}
