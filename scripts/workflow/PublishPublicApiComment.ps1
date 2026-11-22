#Requires -Version 7.6

# Entry point for the trusted workflow_run publisher. The producer runs with read-only access;
# this script runs only from a checkout of the default branch and reconciles its validated
# artifact into a marker-owned PR comment.
# Ref: ../../.github/workflows/design.md#public-api-diff-publication.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string] $ArtifactPath,
    [Parameter(Mandatory)][string] $Repository,
    [Parameter(Mandatory)][long] $PullRequest,
    [Parameter(Mandatory)][long] $RunId,
    [Parameter(Mandatory)][long] $RunAttempt,
    [Parameter(Mandatory)][string] $HeadSha,
    [Parameter(Mandatory)][string] $Conclusion
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

Import-Module (Join-Path $PSScriptRoot 'PublicApiComment.psm1') -Force
$null = Publish-PublicApiComment -ArtifactPath $ArtifactPath -Repository $Repository `
    -PullRequest $PullRequest -RunId $RunId -RunAttempt $RunAttempt -HeadSha $HeadSha `
    -Conclusion $Conclusion
