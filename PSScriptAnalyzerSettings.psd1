# PSScriptAnalyzer configuration for `just validate-scripts` (and the CI script-validation
# step). This is the single source of truth for which rules gate PowerShell in this repo; see
# docs/build-and-tooling.md ("Scripting") for the surrounding convention.
#
# The repo-local custom rules under scripts/analyzer/ (e.g. the foreach case-collision rule that
# would have caught the shadowing bug the default rules miss) are passed via -CustomRulePath by
# the recipe rather than referenced here, so their path is unambiguous regardless of the working
# directory. IncludeDefaultRules keeps the built-in rules running alongside them.
@{
    # Only Error/Warning findings gate. Info-level rules (e.g. PSProvideCommentHelp,
    # PSUseOutputTypeCorrectly) are dropped by this filter, so there is no need to list them in
    # ExcludeRules - this also future-proofs against a rule's default severity changing.
    Severity = @('Error', 'Warning')

    # Run the full built-in rule set in addition to the -CustomRulePath rules. Without this,
    # supplying custom rules would run ONLY the custom rules.
    IncludeDefaultRules = $true

    ExcludeRules = @(
        # Our scripts are developer-tooling / CLI utilities whose whole job is to print progress
        # and results to the console, so Write-Host is a deliberate choice, not an oversight.
        'PSAvoidUsingWriteHost'

        # This repo standardizes on UTF-8 *without* a BOM. Scripts are kept ASCII-only, so this rule
        # would not normally fire; the suppression is a safety net so a stray non-ASCII character can
        # never force a churny, cross-platform-undesirable BOM.
        'PSUseBOMForUnicodeEncodedFile'
    )
}
