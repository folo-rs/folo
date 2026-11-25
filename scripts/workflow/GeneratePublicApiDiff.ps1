#Requires -Version 7.6

# The PR workflow invokes this entry point after environment setup. It computes the
# immutable merge base, selects affected libraries and writes the publisher artifact.
# Ref: ../../.github/workflows/implementation.md#public-api-diff-publication.
param(
    [Parameter(Mandatory)][string] $Repository,
    [Parameter(Mandatory)][long] $PullRequest,
    [Parameter(Mandatory)][string] $BaseSha,
    [Parameter(Mandatory)][string] $HeadSha,
    [Parameter(Mandatory)][long] $RunId,
    [Parameter(Mandatory)][long] $RunAttempt,
    [Parameter(Mandatory)][string] $ArtifactPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

Import-Module "$PSScriptRoot/PublicApiDiff.psm1" -Force
Import-Module "$PSScriptRoot/../build/Delta.psm1" -Force
Import-Module "$PSScriptRoot/../utility/Constants.psm1" -Force

foreach ($sha in @($BaseSha, $HeadSha)) {
    if ($sha -cnotmatch '^[0-9a-f]{40}([0-9a-f]{24})?$') { throw 'Expected a full event commit ID.' }
}
if ((git rev-parse HEAD) -cne $HeadSha) { throw 'Checkout does not match the event head.' }
$mergeBase = git merge-base $BaseSha $HeadSha
$deltaConfig = Join-Path ([IO.Path]::GetTempPath()) "public-api-delta-$([guid]::NewGuid()).toml"
try {
    $config = Get-PublicApiDeltaConfig -Config ([IO.File]::ReadAllText((Resolve-Path 'delta.toml'))) `
        -MergeBaseSha $mergeBase
    [IO.File]::WriteAllText($deltaConfig, $config, [Text.UTF8Encoding]::new($false))
    $affected = @(Invoke-CargoDelta -SkipFetch -IncludeRemovedPackages `
        -BaselineRevision $mergeBase -ConfigPath $deltaConfig)
} finally {
    Remove-Item -LiteralPath $deltaConfig -Force -ErrorAction SilentlyContinue
}
$metadata = cargo metadata --no-deps --format-version 1 --locked | ConvertFrom-Json -AsHashtable
$baselineWorktree = Join-Path ([IO.Path]::GetTempPath()) "public-api-base-$([guid]::NewGuid())"
git worktree add --quiet --detach $baselineWorktree $mergeBase
try {
    Push-Location $baselineWorktree
    try {
        $baselineMetadata = cargo metadata --no-deps --format-version 1 --locked `
            --manifest-path (Join-Path $baselineWorktree 'Cargo.toml') | ConvertFrom-Json -AsHashtable
    } finally {
        Pop-Location
    }
    $packages = @(Select-PublicApiPackage -Affected $affected -Metadata $metadata `
        -BaselineMetadata $baselineMetadata)
    $headLibraries = [Collections.Generic.HashSet[string]]::new(
        [string[]] @(Get-PublicApiLibraryName -Metadata $metadata), [StringComparer]::Ordinal)
    $baselineLibraries = [Collections.Generic.HashSet[string]]::new(
        [string[]] @(Get-PublicApiLibraryName -Metadata $baselineMetadata), [StringComparer]::Ordinal)
    $constants = Read-DotEnvFile -Path 'constants.env'
    $nightly = Get-RequiredConstant -Values $constants -Name 'RUST_NIGHTLY'

    $report = Get-PublicApiReport -Repository $Repository -PullRequest $PullRequest `
        -HeadSha $HeadSha -BaseSha $BaseSha -MergeBaseSha $mergeBase `
        -RunId $RunId -RunAttempt $RunAttempt `
        -Package $packages -Generate {
            param($name)
            $stdout = Join-Path ([IO.Path]::GetTempPath()) "public-api-$([guid]::NewGuid()).out"
            $stderr = "$stdout.err"
            try {
                if ((git rev-parse HEAD) -cne $HeadSha) { throw 'Checkout no longer matches the event head.' }
                $previous = $PSNativeCommandUseErrorActionPreference
                try {
    $PSNativeCommandUseErrorActionPreference = $false
    $hasHeadLibrary = $headLibraries.Contains($name)
    $hasBaselineLibrary = $baselineLibraries.Contains($name)
    if ($hasHeadLibrary -and $hasBaselineLibrary) {
        # The commit diff checks out both revisions in place; keep outputs outside
        # the checkout so they do not prevent those checkouts.
        cargo "+$nightly" public-api -sss --all-features --color never -p $name `
            diff "$mergeBase..$HeadSha" > $stdout 2> $stderr
        $exitCode = $LASTEXITCODE
        $output = [IO.File]::ReadAllText($stdout)
    } elseif ($hasHeadLibrary) {
        # A library target added by this PR has no old API to build at the merge base.
        cargo "+$nightly" public-api -sss --all-features --color never -p $name `
            > $stdout 2> $stderr
        $exitCode = $LASTEXITCODE
        $output = ConvertTo-PublicApiOneSidedDiff `
            -ApiOutput ([IO.File]::ReadAllText($stdout)) -Direction Added
    } else {
        # A library target removed by this PR has no head API to build for the diff.
        Push-Location $baselineWorktree
        try {
            cargo "+$nightly" public-api -sss --all-features --color never -p $name `
                > $stdout 2> $stderr
            $exitCode = $LASTEXITCODE
        } finally {
            Pop-Location
        }
        $output = ConvertTo-PublicApiOneSidedDiff `
            -ApiOutput ([IO.File]::ReadAllText($stdout)) -Direction Removed
    }
                } finally {
    $PSNativeCommandUseErrorActionPreference = $previous
                }
                $errors = [IO.File]::ReadAllText($stderr)
                if ($exitCode -ne 0) { Write-Host "Public API diff failed for $name`:`n$errors" }
                if ((git rev-parse HEAD) -cne $HeadSha) { throw 'Public API tool did not restore the event head.' }
                return @{ Output = $output; ErrorOutput = $errors; ExitCode = $exitCode }
            } finally {
                Remove-Item -LiteralPath $stdout, $stderr -Force -ErrorAction SilentlyContinue
            }
        }

    $json = $report | ConvertTo-Json -Depth 20
    if ([Text.Encoding]::UTF8.GetByteCount($json) -gt 8MB) {
        throw 'Public API report exceeds the publisher artifact size limit.'
    }
    [IO.Directory]::CreateDirectory((Split-Path -Parent $ArtifactPath)) | Out-Null
    [IO.File]::WriteAllText($ArtifactPath, $json, [Text.UTF8Encoding]::new($false))
    if (@($report.packages | Where-Object { $_.status -eq 'error' }).Count -gt 0) {
        throw 'One or more public API diffs failed. See the report and workflow log.'
    }
} finally {
    git worktree remove --force $baselineWorktree
}
