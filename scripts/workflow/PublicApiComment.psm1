#Requires -Version 7.6

# The trusted public-API publisher workflow uses this module to validate an artifact produced
# by untrusted PR code and reconcile one marker-owned GitHub comment. PowerShell is appropriate
# at this boundary because the workflow must remain able to report a failed Rust/tool setup.
# Ref: ../../.github/workflows/design.md#public-api-diff-publication.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

$script:PublicApiCommentMarker = '<!-- folo-public-api-diff:v1 -->'
$script:MaximumArtifactBytes = 8MB
$script:MaximumPackageOutputBytes = 2MB
$script:MaximumCommentBytes = 60KB

function Get-PublicApiJsonProperty {
    # Makes schema-field access explicit while preserving the distinction between a missing
    # field and a present field with a falsey value.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][hashtable] $Object,
        [Parameter(Mandatory)][string] $Name
    )

    if (-not $Object.ContainsKey($Name)) {
        throw "Public API artifact is missing '$Name'."
    }
    $value = $Object[$Name]
    if ($value -is [array]) { return ,$value }
    return $value
}

function Assert-PublicApiText {
    # Public API output crosses a trust boundary into GitHub Markdown. Control characters
    # provide no useful diff content and can make rendering or logging ambiguous, so reject them.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][AllowEmptyString()][string] $Text,
        [Parameter(Mandatory)][string] $Name
    )

    if ($Text -cmatch '[\x00-\x08\x0B\x0C\x0E-\x1F]') {
        throw "Public API artifact field '$Name' contains unsupported control characters."
    }
}

function Assert-PublicApiEventIdentity {
    # These values come from workflow_run expressions, but API endpoints and Markdown links still
    # benefit from rejecting malformed identities before the publisher uses them.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $Repository,
        [Parameter(Mandatory)][long] $PullRequest,
        [Parameter(Mandatory)][long] $RunId,
        [Parameter(Mandatory)][long] $RunAttempt,
        [Parameter(Mandatory)][string] $HeadSha
    )

    if ($Repository -cnotmatch '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$') {
        throw "Workflow repository '$Repository' is not an owner/name identifier."
    }
    if ($PullRequest -le 0 -or $RunId -le 0 -or $RunAttempt -le 0) {
        throw 'Workflow PR, run and attempt identities must be positive integers.'
    }
    if ($HeadSha -cnotmatch '^[0-9a-f]{40}([0-9a-f]{24})?$') {
        throw 'Workflow head_sha must be a full Git object ID.'
    }
}

function Read-PublicApiArtifact {
    # Validates the complete producer/publisher exchange before any GitHub write. The publisher
    # must not infer repository, PR or commit identity from user-controlled report content.
    [CmdletBinding()]
    [OutputType([hashtable])]
    param(
        [Parameter(Mandatory)][string] $Path,
        [Parameter(Mandatory)][string] $Repository,
        [Parameter(Mandatory)][long] $PullRequest,
        [Parameter(Mandatory)][long] $RunId,
        [Parameter(Mandatory)][long] $RunAttempt,
        [Parameter(Mandatory)][string] $HeadSha
    )

    $file = Get-Item -LiteralPath $Path -ErrorAction Stop
    if ($file.Length -gt $script:MaximumArtifactBytes) {
        throw "Public API artifact exceeds the $($script:MaximumArtifactBytes) byte limit."
    }

    $artifact = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json -AsHashtable
    if ($artifact -isnot [hashtable]) {
        throw 'Public API artifact must contain one JSON object.'
    }
    if ((Get-PublicApiJsonProperty $artifact 'schema_version') -isnot [long] -or
        $artifact.schema_version -ne 1) {
        throw 'Public API artifact has an unsupported schema_version.'
    }
    if ((Get-PublicApiJsonProperty $artifact 'repository') -cne $Repository) {
        throw 'Public API artifact repository does not match the workflow repository.'
    }
    foreach ($field in @(
            @{ Name = 'pull_request'; Expected = $PullRequest }
            @{ Name = 'run_id'; Expected = $RunId }
            @{ Name = 'run_attempt'; Expected = $RunAttempt }
        )) {
        $value = Get-PublicApiJsonProperty $artifact $field.Name
        if ($value -isnot [long] -and $value -isnot [int]) {
            throw "Public API artifact field '$($field.Name)' must be an integer."
        }
        if ([long]$value -ne [long]$field.Expected) {
            throw "Public API artifact field '$($field.Name)' does not match the workflow event."
        }
    }
    foreach ($field in @('head_sha', 'merge_base_sha')) {
        $sha = Get-PublicApiJsonProperty $artifact $field
        if ($sha -isnot [string] -or $sha -notmatch '^[0-9a-f]{40}([0-9a-f]{24})?$') {
            throw "Public API artifact field '$field' must be a full Git object ID."
        }
    }
    if ($artifact.head_sha -cne $HeadSha) {
        throw 'Public API artifact head_sha does not match the completed workflow run.'
    }

    $packageValue = Get-PublicApiJsonProperty $artifact 'packages'
    if ($packageValue -isnot [array]) {
        throw "Public API artifact field 'packages' must be an array."
    }

    $names = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    $packages = @(
        foreach ($package in $packageValue) {
            if ($package -isnot [hashtable]) {
                throw 'Every public API package report must be a JSON object.'
            }
            $name = Get-PublicApiJsonProperty $package 'name'
            $status = Get-PublicApiJsonProperty $package 'status'
            $output = Get-PublicApiJsonProperty $package 'output'
            if ($name -isnot [string] -or $name -notmatch '^[A-Za-z0-9_-]+$') {
                throw 'Every public API package report must have a valid Cargo package name.'
            }
            if (-not $names.Add($name)) {
                throw "Public API artifact repeats package '$name'."
            }
            if ($status -isnot [string] -or $status -cnotin @('changed', 'unchanged', 'error')) {
                throw "Public API package '$name' has an unsupported status."
            }
            if ($output -isnot [string]) {
                throw "Public API package '$name' output must be text."
            }
            Assert-PublicApiText -Text $output -Name "packages[$name].output"
            $outputBytes = [Text.Encoding]::UTF8.GetByteCount($output)
            if ($outputBytes -gt $script:MaximumPackageOutputBytes) {
                throw "Public API package '$name' output exceeds the $($script:MaximumPackageOutputBytes) byte limit."
            }
            if ($status -eq 'changed' -and [string]::IsNullOrWhiteSpace($output)) {
                throw "Public API package '$name' is changed but has no diff output."
            }
            @{
                name = $name
                status = $status
                output = $output
            }
        }
    )

    return @{
        schema_version = $artifact.schema_version
        repository = $artifact.repository
        pull_request = [long]$artifact.pull_request
        head_sha = $artifact.head_sha
        merge_base_sha = $artifact.merge_base_sha
        run_id = [long]$artifact.run_id
        run_attempt = [long]$artifact.run_attempt
        packages = $packages
    }
}

function Format-PublicApiCodeFence {
    # A fence longer than every run of backticks in tool output keeps arbitrary Rust tokens
    # from terminating the Markdown code block.
    [CmdletBinding()]
    [OutputType([string])]
    param([Parameter(Mandatory)][AllowEmptyString()][string] $Text)

    $longest = 0
    foreach ($match in [regex]::Matches($Text, '`+')) {
        $longest = [Math]::Max($longest, $match.Length)
    }
    return ('`' * [Math]::Max(3, $longest + 1))
}

function ConvertTo-PublicApiComment {
    # Renders only validated structured fields. Tool output stays inside a dynamically sized
    # code fence, so report text cannot create comment headings, mentions or the ownership marker.
    [CmdletBinding()]
    [OutputType([string])]
    param(
        [Parameter(Mandatory)][hashtable] $Artifact,
        [Parameter(Mandatory)][long] $RunId,
        [Parameter(Mandatory)][long] $RunAttempt,
        [long] $MaximumBytes = $script:MaximumCommentBytes
    )

    $commitUrl = "https://github.com/$($Artifact.repository)/commit/$($Artifact.head_sha)"
    $runUrl = "https://github.com/$($Artifact.repository)/actions/runs/$RunId"
    if ($RunAttempt -gt 1) {
        $runUrl += "/attempts/$RunAttempt"
    }

    $lines = [System.Collections.Generic.List[string]]::new()
    $lines.Add($script:PublicApiCommentMarker)
    $lines.Add('## Public API diff')
    $lines.Add('')
    $lines.Add("Generated for commit [``$($Artifact.head_sha.Substring(0, 12))``]($commitUrl) by [Public API diff]($runUrl).")

    $changed = @($Artifact.packages | Where-Object { $_.status -eq 'changed' })
    $errors = @($Artifact.packages | Where-Object { $_.status -eq 'error' })
    if ($changed.Count -eq 0 -and $errors.Count -eq 0) {
        $lines.Add('')
        $lines.Add('No public API changes.')
    } else {
        foreach ($package in $changed) {
            $text = $package.output.Trim()
            $fence = Format-PublicApiCodeFence -Text $text
            $lines.Add('')
            $lines.Add("### ``$($package.name)``")
            $lines.Add('')
            $lines.Add($fence + 'text')
            $lines.Add($text)
            $lines.Add($fence)
        }
        if ($errors.Count -gt 0) {
            $lines.Add('')
            $lines.Add('API diff generation was incomplete for the following packages. See the workflow run for diagnostics:')
            foreach ($package in $errors) {
                $lines.Add("- ``$($package.name)``")
            }
        }
    }

    $body = $lines -join "`n"
    if ([Text.Encoding]::UTF8.GetByteCount($body) -le $MaximumBytes) {
        return $body
    }

    # GitHub caps comment bodies. Preserve the ownership marker, scope and authoritative run
    # link rather than risking a rejected write or silently dropping package evidence.
    $packageList = @($changed | ForEach-Object { "``$($_.name)``" }) -join ', '
    if ([string]::IsNullOrEmpty($packageList)) { $packageList = 'none' }
    return @(
        $script:PublicApiCommentMarker
        '## Public API diff'
        ''
        "Generated for commit [``$($Artifact.head_sha.Substring(0, 12))``]($commitUrl) by [Public API diff]($runUrl)."
        ''
        "The complete diff exceeds the GitHub comment size limit. View the workflow artifact. Changed packages: $packageList."
    ) -join "`n"
}

function ConvertTo-PublicApiFailureComment {
    # The trusted publisher retains a visible explanation when no valid producer artifact
    # exists. Identity here comes only from validated workflow inputs, never from a report body.
    [CmdletBinding()]
    [OutputType([string])]
    param(
        [Parameter(Mandatory)][string] $Repository,
        [Parameter(Mandatory)][long] $RunId,
        [Parameter(Mandatory)][long] $RunAttempt,
        [Parameter(Mandatory)][string] $HeadSha,
        [Parameter(Mandatory)][string] $Detail
    )

    $commitUrl = "https://github.com/$Repository/commit/$HeadSha"
    $runUrl = "https://github.com/$Repository/actions/runs/$RunId"
    if ($RunAttempt -gt 1) {
        $runUrl += "/attempts/$RunAttempt"
    }
    return @(
        $script:PublicApiCommentMarker
        '## Public API diff'
        ''
        "Generated for commit [``$($HeadSha.Substring(0, 12))``]($commitUrl) by [Public API diff]($runUrl)."
        ''
        "Public API diff generation did not complete. $Detail See the workflow run for diagnostics."
    ) -join "`n"
}

function Invoke-PublicApiGitHubJson {
    # Runs one GitHub request with complete JSON transport. Writes are deliberately not retried:
    # callers reconcile an ambiguous outcome before deciding that it failed.
    [CmdletBinding()]
    [OutputType([object])]
    param(
        [Parameter(Mandatory)][string] $Endpoint,
        [ValidateSet('GET', 'POST', 'PATCH')][string] $Method = 'GET',
        [hashtable] $Body
    )

    $arguments = @('api', $Endpoint, '--method', $Method)
    $PSNativeCommandUseErrorActionPreference = $false
    $output = @(if ($null -ne $Body) {
            $Body | ConvertTo-Json -Depth 20 -Compress | & gh @arguments --input - 2>&1
        } else {
            & gh @arguments 2>&1
        })
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw "GitHub API request failed (exit $exitCode): $($arguments -join ' '): $($output -join "`n")"
    }
    $text = ($output | Where-Object { $_ -isnot [Management.Automation.ErrorRecord] }) -join "`n"
    if ([string]::IsNullOrWhiteSpace($text)) { return $null }
    return ,($text | ConvertFrom-Json -AsHashtable -NoEnumerate)
}

function Get-PublicApiGitHubCollection {
    # gh owns pagination. `--slurp` preserves page boundaries, allowing this module to flatten
    # complete collections without a potentially non-progressing page loop.
    [CmdletBinding()]
    [OutputType([object[]])]
    param([Parameter(Mandatory)][string] $Endpoint)

    $separator = if ($Endpoint.Contains('?')) { '&' } else { '?' }
    $arguments = @(
        'api'
        "$Endpoint${separator}per_page=100"
        '--paginate'
        '--slurp'
    )
    $PSNativeCommandUseErrorActionPreference = $false
    $output = @(& gh @arguments 2>&1)
    if ($LASTEXITCODE -ne 0) {
        throw "GitHub collection read failed (exit $LASTEXITCODE): $($output -join "`n")"
    }
    $text = ($output | Where-Object { $_ -isnot [Management.Automation.ErrorRecord] }) -join "`n"
    $pages = @($text | ConvertFrom-Json -AsHashtable -NoEnumerate)
    return @(
        foreach ($page in $pages) {
            if ($page -isnot [array]) {
                throw "GitHub collection endpoint '$Endpoint' returned a non-array page."
            }
            foreach ($item in $page) { $item }
        }
    )
}

function Assert-PublicApiLiveHead {
    # A completed run can become stale while its publisher waits. Never let an older run replace
    # the comment for the current PR head.
    [CmdletBinding()]
    [OutputType([hashtable])]
    param(
        [Parameter(Mandatory)][string] $Repository,
        [Parameter(Mandatory)][long] $PullRequest,
        [Parameter(Mandatory)][string] $HeadSha
    )

    $pull = Invoke-PublicApiGitHubJson "repos/$Repository/pulls/$PullRequest"
    if ($pull -isnot [hashtable] -or
        $pull.head -isnot [hashtable] -or
        $pull.head.sha -isnot [string] -or
        $pull.head.sha -cne $HeadSha) {
        throw "Pull request $Repository#$PullRequest has advanced beyond workflow head $HeadSha."
    }
    return $pull
}

function Publish-PublicApiPullRequestComment {
    # Marker ownership makes the write idempotent without searching PR descriptions or history.
    # A duplicate marker is an ambiguous repository state, not permission to overwrite one copy.
    [CmdletBinding()]
    [OutputType([hashtable])]
    param(
        [Parameter(Mandatory)][string] $Repository,
        [Parameter(Mandatory)][long] $PullRequest,
        [Parameter(Mandatory)][string] $HeadSha,
        [Parameter(Mandatory)][string] $Body
    )

    if ([Text.Encoding]::UTF8.GetByteCount($Body) -gt 65536) {
        throw 'Public API comment exceeds the GitHub body limit.'
    }
    $null = Assert-PublicApiLiveHead -Repository $Repository -PullRequest $PullRequest -HeadSha $HeadSha
    $endpoint = "repos/$Repository/issues/$PullRequest/comments"
    $comments = @(Get-PublicApiGitHubCollection $endpoint)
    $owned = @($comments | Where-Object { $_.body -is [string] -and $_.body.Contains($script:PublicApiCommentMarker) })
    if ($owned.Count -gt 1) {
        throw "Pull request $Repository#$PullRequest has multiple public API diff marker comments."
    }

    $method = 'POST'
    $writeEndpoint = $endpoint
    if ($owned.Count -eq 1) {
        $existing = $owned[0]
        if ($existing.body -ceq $Body) { return $existing }
        $method = 'PATCH'
        $writeEndpoint = "repos/$Repository/issues/comments/$($existing.id)"
    }

    # Revalidate immediately before mutation because a newer producer run can advance the PR
    # while this publisher reads the comment collection.
    $null = Assert-PublicApiLiveHead -Repository $Repository -PullRequest $PullRequest -HeadSha $HeadSha
    try {
        return Invoke-PublicApiGitHubJson -Endpoint $writeEndpoint -Method $method -Body @{ body = $Body }
    } catch {
        $writeFailure = $_
        # Reconcile a potentially committed but ambiguous request by reading the marker comment.
        try {
            $persisted = @(Get-PublicApiGitHubCollection $endpoint | Where-Object {
                    $_.body -is [string] -and $_.body.Contains($script:PublicApiCommentMarker)
                })
            if ($persisted.Count -eq 1 -and $persisted[0].body -ceq $Body) { return $persisted[0] }
        } catch {
            throw [InvalidOperationException]::new(
                "Could not reconcile public API comment write: $($_.Exception.Message)",
                $writeFailure.Exception)
        }
        throw $writeFailure
    }
}

function Publish-PublicApiComment {
    # Orchestrates artifact validation and marker-comment reconciliation. Invalid or absent
    # artifacts still produce a useful failure comment; contract violations then fail the
    # publisher job, while a producer workflow failure is already represented by that run.
    [CmdletBinding()]
    [OutputType([hashtable])]
    param(
        [Parameter(Mandatory)][string] $ArtifactPath,
        [Parameter(Mandatory)][string] $Repository,
        [Parameter(Mandatory)][long] $PullRequest,
        [Parameter(Mandatory)][long] $RunId,
        [Parameter(Mandatory)][long] $RunAttempt,
        [Parameter(Mandatory)][string] $HeadSha,
        [Parameter(Mandatory)][string] $Conclusion
    )

    Assert-PublicApiEventIdentity -Repository $Repository -PullRequest $PullRequest `
        -RunId $RunId -RunAttempt $RunAttempt -HeadSha $HeadSha
    $contractFailure = $null
    if (-not (Test-Path -LiteralPath $ArtifactPath -PathType Leaf)) {
        if ($Conclusion -eq 'success') {
            $contractFailure = [InvalidOperationException]::new(
                'The successful Public API diff workflow did not publish its required artifact.')
        }
        $body = ConvertTo-PublicApiFailureComment -Repository $Repository -RunId $RunId `
            -RunAttempt $RunAttempt -HeadSha $HeadSha -Detail 'The producer did not publish a report.'
    } else {
        try {
            $artifact = Read-PublicApiArtifact -Path $ArtifactPath -Repository $Repository `
                -PullRequest $PullRequest -RunId $RunId -RunAttempt $RunAttempt -HeadSha $HeadSha
            $body = ConvertTo-PublicApiComment -Artifact $artifact -RunId $RunId `
                -RunAttempt $RunAttempt
        } catch {
            $contractFailure = $_.Exception
            $body = ConvertTo-PublicApiFailureComment -Repository $Repository -RunId $RunId `
                -RunAttempt $RunAttempt -HeadSha $HeadSha -Detail 'The producer artifact was invalid.'
        }
    }

    $comment = Publish-PublicApiPullRequestComment -Repository $Repository -PullRequest $PullRequest `
        -HeadSha $HeadSha -Body $body
    if ($null -ne $contractFailure) { throw $contractFailure }
    return $comment
}

Export-ModuleMember -Function @(
    'ConvertTo-PublicApiComment'
    'ConvertTo-PublicApiFailureComment'
    'Publish-PublicApiComment'
    'Read-PublicApiArtifact'
    'Publish-PublicApiPullRequestComment'
)
