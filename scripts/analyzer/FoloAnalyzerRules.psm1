Set-StrictMode -Version Latest

# Repo-local PSScriptAnalyzer custom rules. PSScriptAnalyzer discovers each rule by convention:
# an advanced function named `Measure-*` that takes a typed AST parameter and returns
# DiagnosticRecord objects. The recipe `just validate-scripts` passes this module via
# -CustomRulePath -IncludeDefaultRules, so these rules run alongside the built-in rules.
#
# This module exists because the built-in rule set (and Set-StrictMode) miss whole classes of
# real bugs specific to how we write PowerShell - most notably the case-insensitive variable
# collision that shipped a broken release (a `foreach` whose loop variable name only differed in
# case from the collection it enumerated, silently clobbering the collection to its last
# element). Grow this module as we discover new footguns worth catching mechanically.

<#
.SYNOPSIS
    Flags a `foreach` whose loop variable case-insensitively collides with the variable it
    enumerates (e.g. `foreach ($target in $Target)`).
.DESCRIPTION
    PowerShell variable names are case-insensitive, so `$target` and `$Target` are the SAME
    variable. Writing `foreach ($target in $Target) { ... }` therefore overwrites the collection
    on every iteration; after the loop the collection variable holds only its last element. This
    is a silent, high-impact bug that no built-in PSScriptAnalyzer rule catches, so we catch it
    here as an Error.

    The parameter is typed as [ForEachStatementAst] so PSScriptAnalyzer invokes the rule once per
    matching node - typing it as the broad [ScriptBlockAst] and recursing with FindAll would
    report the same statement multiple times (once per enclosing scope).
#>
function Measure-ForeachVariableShadowsSource {
    [CmdletBinding()]
    [OutputType([Microsoft.Windows.PowerShell.ScriptAnalyzer.Generic.DiagnosticRecord[]])]
    param(
        [Parameter(Mandatory)]
        [System.Management.Automation.Language.ForEachStatementAst]$ForEachStatementAst
    )

    $loopVariableName = $ForEachStatementAst.Variable.VariablePath.UserPath

    # The condition is the enumerated expression. Unwrap the common `pipeline -> command
    # expression -> variable` nesting to reach a bare `$Foo` source; anything more complex (a
    # method call, an array literal, a sub-expression) cannot be a plain variable collision, so we
    # only act on a simple VariableExpressionAst.
    $sourceExpression = $ForEachStatementAst.Condition
    if ($sourceExpression -is [System.Management.Automation.Language.PipelineAst] -and
        $sourceExpression.PipelineElements.Count -eq 1) {
        $sourceExpression = $sourceExpression.PipelineElements[0]
    }
    if ($sourceExpression -is [System.Management.Automation.Language.CommandExpressionAst]) {
        $sourceExpression = $sourceExpression.Expression
    }

    if ($sourceExpression -isnot [System.Management.Automation.Language.VariableExpressionAst]) {
        return
    }

    $sourceVariableName = $sourceExpression.VariablePath.UserPath
    if ($loopVariableName -and $sourceVariableName -and ($loopVariableName -ieq $sourceVariableName)) {
        [Microsoft.Windows.PowerShell.ScriptAnalyzer.Generic.DiagnosticRecord]@{
            Message  = "foreach loop variable '`$$loopVariableName' case-insensitively collides " +
                "with the enumerated variable '`$$sourceVariableName' - they are the SAME " +
                "variable, so the collection is clobbered to its last element. Rename the loop " +
                "variable (e.g. '`$item')."
            Extent   = $ForEachStatementAst.Extent
            RuleName = 'FoloAvoidForeachVariableShadowsSource'
            Severity = 'Error'
        }
    }
}

Export-ModuleMember -Function Measure-ForeachVariableShadowsSource
