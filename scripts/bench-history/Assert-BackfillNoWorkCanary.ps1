#Requires -Version 7.6

# The hosted caller canary uses current-attempt job evidence to prove that a successful
# no-eligible preparation did not start benchmark work. This is test orchestration, not
# an output or credential requirement of the public backfill workflow.
# Ref: .github/workflows/implementation.md#reusable-workflow-canary.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string] $Repository,
    [Parameter(Mandatory)][ValidateRange(1, [long]::MaxValue)][long] $RunId,
    [Parameter(Mandatory)][ValidateRange(1, [int]::MaxValue)][int] $RunAttempt,
    [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string] $OutputPath
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

$json = gh api --paginate --slurp "repos/$Repository/actions/runs/$RunId/attempts/$RunAttempt/jobs?per_page=100"
$json | Set-Content -LiteralPath $OutputPath -Encoding utf8
$pages = $json | ConvertFrom-Json
$jobs = @($pages.jobs | Where-Object { $_.name.StartsWith('no-eligible-backfill / ', [StringComparison]::Ordinal) })
$preparation = @($jobs | Where-Object { $_.name -ceq 'no-eligible-backfill / prepare' })
if ($preparation.Count -ne 1 -or $preparation[0].conclusion -cne 'success') {
    throw 'The no-eligible canary must have successful preparation in this run attempt.'
}
foreach ($job in $jobs) {
    if ($job.name -ceq 'no-eligible-backfill / prepare') { continue }
    if ($job.conclusion -cne 'skipped') {
        throw "The no-eligible canary executed unexpected work: $($job.name)."
    }
}
