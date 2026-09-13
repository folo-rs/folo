#requires -Version 7
# Records real Just argument binding for ScheduledExecution.Tests.ps1 without running a checker.
# Optional source copying exercises isolation while the parent holds its capture streams open.
param([string] $Recipe, [AllowEmptyString()][string] $Package)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

@{
    recipe = $Recipe
    package = $Package
    shard = $env:SHARD
    careful = $env:CAREFUL
    output = $env:OUTPUT
} | ConvertTo-Json | Set-Content -LiteralPath $env:SCHEDULED_CAPTURE_PATH

if ($Recipe -eq 'mutants') {
    if ($env:SCHEDULED_SOURCE_COPY) {
        $null = New-Item -ItemType Directory -Path $env:SCHEDULED_SOURCE_COPY
        Get-ChildItem -LiteralPath (Get-Location).Path -Force |
            Copy-Item -Destination $env:SCHEDULED_SOURCE_COPY -Recurse -Force
        [Console]::Out.WriteLine('Source copy finished.')
    }
    $null = New-Item -ItemType Directory -Path (Join-Path $env:OUTPUT 'mutants.out') -Force
    if ($env:SCHEDULED_MUTATION_FIXTURE) {
        Copy-Item -LiteralPath $env:SCHEDULED_MUTATION_FIXTURE `
            -Destination (Join-Path $env:OUTPUT 'mutants.out\outcomes.json')
    } else {
        '[]' | Set-Content -LiteralPath (Join-Path $env:OUTPUT 'mutants.out\mutants.json')
    }
}
if ([int]$env:SCHEDULED_TEST_EXIT -ne 0) {
    [Console]::Error.WriteLine('Recipe failure canary.')
}
exit [int]$env:SCHEDULED_TEST_EXIT
