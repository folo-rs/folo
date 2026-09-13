#requires -Version 7

# Deep validation invokes this entrypoint in its main checkout and propagates its exit code.
# The matrix supplies a Just recipe and its scope; GitHub supplies the run's commit.
# Summary.md feeds reporting.
# Ref: .github/workflows/implementation.md#deep-execution.
[CmdletBinding()]
param(
    [string] $CheckJson = $env:SCHEDULED_CHECK,
    [string] $SourceSha = $env:GITHUB_SHA,
    [string] $SourceRoot = '.',
    # Local runs retain separate diagnostics outside source; CI supplies its upload location.
    # Ref: .github/workflows/implementation.md#deep-execution.
    [string] $OutputDirectory = (Join-Path ([IO.Path]::GetTempPath()) ("folo-scheduled-result-" + [guid]::NewGuid()))
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'
Import-Module (Join-Path $PSScriptRoot 'ScheduledExecution.psm1') -Force
$check = ConvertFrom-Json -InputObject $CheckJson -AsHashtable
$exitCode = Invoke-ScheduledCheck -Check $check -SourceRoot ([IO.Path]::GetFullPath($SourceRoot)) `
    -OutputDirectory ([IO.Path]::GetFullPath($OutputDirectory)) -SourceSha $SourceSha
exit $exitCode
