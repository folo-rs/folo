#Requires -Version 7.6

# The read-only PR workflow uses this module to select affected libraries and build the
# publisher's report. PowerShell can still report tool failures without compiling a helper.
# Ref: ../../.github/workflows/implementation.md#public-api-diff-publication.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

function Get-PublicApiDeltaConfig {
    # Delta's snapshots do not set its Git comparison ref; both inputs must use the same
    # merge base or an inherited trip wire can select the wrong packages for a stacked PR.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $Config,
        [Parameter(Mandatory)][string] $MergeBaseSha
    )

    if ($MergeBaseSha -cnotmatch '^[0-9a-f]{40}([0-9a-f]{24})?$') {
        throw 'Expected a full merge-base commit ID.'
    }
    $pattern = '(?m)^remote_branch\s*=\s*"[^"\r\n]*"\s*$'
    if ([regex]::Matches($Config, $pattern).Count -ne 1) {
        throw 'Delta configuration must contain exactly one remote_branch setting.'
    }
    return [regex]::Replace($Config, $pattern, "remote_branch = `"$MergeBaseSha`"")
}

function Select-PublicApiPackage {
    # Cargo metadata supplies current workspace membership, publication policy and target kinds.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][AllowEmptyCollection()][string[]] $Affected,
        [Parameter(Mandatory)][hashtable] $Metadata
    )

    $members = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($id in $Metadata.workspace_members) { $null = $members.Add($id) }
    $affectedNames = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($name in $Affected) { $null = $affectedNames.Add($name) }
    foreach ($package in $Metadata.packages) {
        if ($members.Contains($package.id) -and $affectedNames.Contains($package.name) -and
            ($null -eq $package.publish -or $package.publish.Count -gt 0) -and
            @($package.targets | Where-Object {
                @($_.kind | Where-Object { $_ -in @('lib', 'rlib', 'dylib', 'cdylib', 'staticlib', 'proc-macro') }).Count -gt 0
            }).Count -gt 0) {
            $package.name
        }
    }
}

function ConvertTo-PublicApiAddedDiff {
    # A library created by the PR has no old API to build. Present its head API as additions
    # so the report uses the same visible diff format as cargo-public-api's commit comparison.
    [CmdletBinding()]
    [OutputType([string])]
    param([Parameter(Mandatory)][AllowEmptyString()][string] $ApiOutput)

    $items = @($ApiOutput -split '\r?\n' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    if ($items.Count -eq 0) { $items = @('(none)') }
    else { $items = @($items | ForEach-Object { "+$_" }) }
    return @(
        'Removed items from the public API'
        '================================='
        '(none)'
        ''
        'Changed items in the public API'
        '==============================='
        '(none)'
        ''
        'Added items to the public API'
        '============================='
        $items
    ) -join "`n"
}

function ConvertTo-PublicApiPackageReport {
    # The pinned tool prints headings even for an empty diff. Require its complete empty
    # representation before declaring no change; unrecognized output is a visible failure.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $Name,
        [Parameter(Mandatory)][AllowEmptyString()][string] $Output,
        [Parameter(Mandatory)][int] $ExitCode,
        [AllowEmptyString()][string] $ErrorOutput = ''
    )

    $empty = '(?s)\A\s*Removed items from the public API\s*=+\s*\(none\)\s*' +
        'Changed items in the public API\s*=+\s*\(none\)\s*' +
        'Added items to the public API\s*=+\s*\(none\)\s*\z'
    $status = if ($ExitCode -ne 0) {
        'error'
    } elseif ($Output -match $empty) {
        'unchanged'
    } elseif ($Output -match '(?m)^[+-]') {
        'changed'
    } else {
        'error'
    }
    $text = if ($status -eq 'error') { "$Output`n$ErrorOutput".Trim() } else { $Output }
    # The trusted publisher rejects unsafe controls and enforces a per-package byte limit.
    if ($text -cmatch '[\x00-\x08\x0B\x0C\x0E-\x1F]' -or
        [Text.Encoding]::UTF8.GetByteCount($text) -gt 2MB) {
        $status = 'error'
        $text = 'Output contains unsupported control characters or exceeds the artifact limit; see the workflow log.'
    }
    if ($status -eq 'error' -and [string]::IsNullOrWhiteSpace($text)) {
        $text = 'Public API diff generation failed or produced unrecognized output.'
    }
    return @{ name = $Name; status = $status; output = $text }
}

function Get-PublicApiReport {
    # The callback boundary keeps selection, classification and artifact shaping testable
    # without invoking Cargo or Git; all package failures are represented before failing the job.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $Repository,
        [Parameter(Mandatory)][long] $PullRequest,
        [Parameter(Mandatory)][string] $HeadSha,
        [Parameter(Mandatory)][string] $MergeBaseSha,
        [Parameter(Mandatory)][long] $RunId,
        [Parameter(Mandatory)][long] $RunAttempt,
        [Parameter(Mandatory)][AllowEmptyCollection()][string[]] $Package,
        [Parameter(Mandatory)][scriptblock] $Generate
    )

    $packages = @(
        foreach ($name in $Package) {
            try {
                $result = & $Generate $name
                ConvertTo-PublicApiPackageReport -Name $name -Output $result.Output `
                    -ExitCode $result.ExitCode -ErrorOutput $result.ErrorOutput
            } catch {
                ConvertTo-PublicApiPackageReport -Name $name -Output '' -ExitCode 1 `
                    -ErrorOutput $_.Exception.Message
            }
        }
    )
    return @{
        schema_version = 1
        repository = $Repository
        pull_request = $PullRequest
        head_sha = $HeadSha
        merge_base_sha = $MergeBaseSha
        run_id = $RunId
        run_attempt = $RunAttempt
        packages = $packages
    }
}

Export-ModuleMember -Function Get-PublicApiDeltaConfig, Select-PublicApiPackage, `
    ConvertTo-PublicApiAddedDiff, ConvertTo-PublicApiPackageReport, Get-PublicApiReport
