#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

Set-StrictMode -Version Latest

# Pester suite for the repo-local PSScriptAnalyzer custom rules in FoloAnalyzerRules.psm1. Each
# case runs the real analyzer over a script snippet with only our custom rule enabled, so the
# tests exercise the rule exactly as `just validate-scripts` invokes it. The headline guarantee is
# that the foreach case-collision that shipped a broken release is now flagged as an Error - and
# that legitimately-distinct loop variables stay clean (no false positives).

BeforeAll {
    Import-Module PSScriptAnalyzer -ErrorAction Stop

    $script:RuleModule = Join-Path $PSScriptRoot 'FoloAnalyzerRules.psm1'
    $script:RuleName = 'Measure-ForeachVariableShadowsSource'

    function Invoke-Rule {
        param([Parameter(Mandatory)][string]$Script)
        # Streams the analyzer's DiagnosticRecord objects. Callers wrap the result in @(...) so that
        # `.Count`/indexing is safe under Set-StrictMode even for zero or one finding (a function
        # return unwraps a single-element result to a scalar and an empty one to $null).
        Invoke-ScriptAnalyzer -ScriptDefinition $Script `
            -CustomRulePath $script:RuleModule `
            -IncludeRule $script:RuleName
    }
}

Describe 'Measure-ForeachVariableShadowsSource' {
    It 'flags a foreach whose loop variable only differs in case from the collection' {
        $findings = @(Invoke-Rule -Script 'foreach ($target in $Target) { $target }')
        $findings.Count | Should -Be 1
        $findings[0].Severity | Should -Be 'Error'
        $findings[0].RuleName | Should -Be 'FoloAvoidForeachVariableShadowsSource'
        $findings[0].Message | Should -Match 'clobbered to its last element'
    }

    It 'flags a foreach whose loop variable is identical to the collection' {
        @(Invoke-Rule -Script 'foreach ($Target in $Target) { $Target }').Count | Should -Be 1
    }

    It 'reports each occurrence exactly once (no per-scope duplication)' {
        $script = @'
function Outer {
    param([string[]]$Target)
    foreach ($target in $Target) { $target }
}
'@
        @(Invoke-Rule -Script $script).Count | Should -Be 1
    }

    It 'flags every colliding loop, and only those' {
        $script = @'
foreach ($item in $Items) { $item }
foreach ($target in $Target) { $target }
foreach ($node in $Nodes) { $node }
'@
        @(Invoke-Rule -Script $script).Count | Should -Be 1
    }

    It 'does not flag distinct loop and collection variable names' {
        @(Invoke-Rule -Script 'foreach ($releaseTarget in $Target) { $releaseTarget }').Count | Should -Be 0
    }

    It 'does not flag enumeration over an array literal' {
        @(Invoke-Rule -Script 'foreach ($x in @(1, 2, 3)) { $x }').Count | Should -Be 0
    }

    It 'does not flag enumeration over a method or command result' {
        @(Invoke-Rule -Script 'foreach ($line in (Get-Content $path)) { $line }').Count | Should -Be 0
    }
}
