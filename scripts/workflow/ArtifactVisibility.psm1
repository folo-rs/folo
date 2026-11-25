#Requires -Version 7.6

# Owns the authenticated, bounded visibility gate between an artifact upload and the official
# download action. The gate establishes complete same-run metadata and rerun freshness, then leaves
# archive retrieval and extraction to the maintained first-party action.
# Ref: .github/workflows/implementation.md#same-run-artifact-visibility.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

Import-Module (Join-Path $PSScriptRoot '..\utility\Retry.psm1') -Force

$script:MaximumAttempts = 12
$script:RetryDelaySeconds = 5
$script:RequestTimeoutSeconds = 30
$script:TransientHttpStatuses = @(404, 408, 429, 500, 502, 503, 504)

function Get-GitHubArtifactHttpStatusCode {
    # Reads only explicit HTTP status data; transport and parsing failures keep their own type.
    [CmdletBinding()]
    [OutputType([int])]
    param(
        [Parameter(Mandatory)] [System.Management.Automation.ErrorRecord] $ErrorRecord
    )

    $responseProperty = $ErrorRecord.Exception.PSObject.Properties['Response']
    if ($null -eq $responseProperty -or $null -eq $responseProperty.Value) { return $null }
    $statusProperty = $responseProperty.Value.PSObject.Properties['StatusCode']
    if ($null -eq $statusProperty) { return $null }
    return [int] $statusProperty.Value
}

function Get-GitHubArtifactResponseProperty {
    # Preserves empty collection properties instead of pipeline-enumerating [] into $null.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)] $Response,
        [Parameter(Mandatory)] [string] $Name
    )

    $found = $false
    $property = $null
    if ($Response -is [System.Collections.IDictionary]) {
        if ($Response.Contains($Name)) {
            $found = $true
            $property = $Response[$Name]
        }
    }
    else {
        $member = $Response.PSObject.Properties[$Name]
        if ($null -ne $member) {
            $found = $true
            $property = $member.Value
        }
    }

    if (-not $found -or $null -eq $property) {
        throw "GitHub artifact response omitted required property '$Name'."
    }
    return $property
}

function Get-GitHubArtifactPendingError {
    # Missing metadata is deterministic retry evidence rather than an API defect.
    [CmdletBinding()]
    [OutputType([System.Management.Automation.ErrorRecord])]
    param(
        [Parameter(Mandatory)] [string] $Message
    )

    return [System.Management.Automation.ErrorRecord]::new(
        [System.InvalidOperationException]::new($Message),
        'ArtifactVisibilityPending',
        [System.Management.Automation.ErrorCategory]::ResourceUnavailable,
        $null
    )
}

function Get-GitHubArtifactTransientError {
    # Marks only explicitly transient HTTP classes so permanent 4xx responses propagate unchanged.
    [CmdletBinding()]
    [OutputType([System.Management.Automation.ErrorRecord])]
    param(
        [Parameter(Mandatory)] [string] $Message,
        [Parameter(Mandatory)] [System.Exception] $InnerException
    )

    return [System.Management.Automation.ErrorRecord]::new(
        [System.InvalidOperationException]::new($Message, $InnerException),
        'ArtifactRequestTransient',
        [System.Management.Automation.ErrorCategory]::ConnectionError,
        $null
    )
}

function Invoke-GitHubArtifactRestMethod {
    # Applies a hard per-request timeout and classifies only bounded retry categories.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)] [uri] $Uri,
        [Parameter(Mandatory)] [hashtable] $Headers,
        [Parameter(Mandatory)] [ValidateRange(1, 300)] [int] $TimeoutSeconds
    )

    try {
        return Invoke-RestMethod -Uri $Uri -Headers $Headers -TimeoutSec $TimeoutSeconds -ErrorAction Stop
    }
    catch {
        $status = Get-GitHubArtifactHttpStatusCode -ErrorRecord $_
        if ($null -ne $status -and ($status -eq 408 -or $status -eq 429 -or $status -ge 500)) {
            throw (Get-GitHubArtifactTransientError `
                    -Message "GitHub artifact request failed with HTTP $status." `
                    -InnerException $_.Exception)
        }
        throw
    }
}

function Get-GitHubRunArtifact {
    # Reads the complete paginated run inventory because the official download action resolves
    # same-run IDs and names through this authenticated inventory.
    [CmdletBinding()]
    [OutputType([pscustomobject])]
    param(
        [Parameter(Mandatory)] [uri] $ApiUrl,
        [Parameter(Mandatory)] [string] $Repository,
        [Parameter(Mandatory)] [long] $RunId,
        [Parameter(Mandatory)] [hashtable] $Headers,
        [Parameter(Mandatory)] [int] $TimeoutSeconds
    )

    $artifacts = @()
    $expectedCount = $null
    $page = 1
    $base = $ApiUrl.AbsoluteUri.TrimEnd('/')
    while ($true) {
        $uri = "$base/repos/$Repository/actions/runs/$RunId/artifacts?per_page=100&page=$page"
        try {
            $response = Invoke-GitHubArtifactRestMethod -Uri $uri -Headers $Headers `
                -TimeoutSeconds $TimeoutSeconds
        }
        catch {
            if ((Get-GitHubArtifactHttpStatusCode -ErrorRecord $_) -eq 404) { return @() }
            throw
        }
        if ($null -eq $expectedCount) {
            $expectedCount = [int] (Get-GitHubArtifactResponseProperty `
                    -Response $response -Name 'total_count')
        }
        $batch = @(Get-GitHubArtifactResponseProperty -Response $response -Name 'artifacts')
        $artifacts += $batch
        if ($artifacts.Count -ge $expectedCount) { return @($artifacts) }
        if ($batch.Count -eq 0) {
            throw "GitHub returned an incomplete artifact inventory for workflow run $RunId."
        }
        $page++
    }
}

function Get-GitHubRunAttemptJob {
    # Reads current-attempt jobs so producer freshness distinguishes actual reruns from inherited
    # successful jobs whose artifacts must remain valid.
    [CmdletBinding()]
    [OutputType([pscustomobject])]
    param(
        [Parameter(Mandatory)] [uri] $ApiUrl,
        [Parameter(Mandatory)] [string] $Repository,
        [Parameter(Mandatory)] [long] $RunId,
        [Parameter(Mandatory)] [int] $RunAttempt,
        [Parameter(Mandatory)] [hashtable] $Headers,
        [Parameter(Mandatory)] [int] $TimeoutSeconds
    )

    $jobs = @()
    $expectedCount = $null
    $page = 1
    $base = $ApiUrl.AbsoluteUri.TrimEnd('/')
    while ($true) {
        $uri = "$base/repos/$Repository/actions/runs/$RunId/attempts/$RunAttempt/jobs" +
            "?per_page=100&page=$page"
        $response = Invoke-GitHubArtifactRestMethod -Uri $uri -Headers $Headers `
            -TimeoutSeconds $TimeoutSeconds
        if ($null -eq $expectedCount) {
            $expectedCount = [int] (Get-GitHubArtifactResponseProperty `
                    -Response $response -Name 'total_count')
        }
        $batch = @(Get-GitHubArtifactResponseProperty -Response $response -Name 'jobs')
        $jobs += $batch
        if ($jobs.Count -ge $expectedCount) { return @($jobs) }
        if ($batch.Count -eq 0) {
            throw "GitHub returned an incomplete job inventory for workflow attempt $RunAttempt."
        }
        $page++
    }
}

function Get-GitHubArtifactJobMarker {
    # Reusable workflows prefix local job names; producer matching uses the final local component.
    [CmdletBinding()]
    [OutputType([string])]
    param(
        [Parameter(Mandatory)] $Job
    )

    $name = [string] (Get-GitHubArtifactResponseProperty -Response $Job -Name 'name')
    $separator = $name.LastIndexOf(' / ', [StringComparison]::Ordinal)
    if ($separator -lt 0) { return $name }
    return $name.Substring($separator + 3)
}

function Select-GitHubRunArtifact {
    # Resolves latest-per-name selection to match download-artifact behavior while keeping one ID
    # exact. Pattern consumers select only their explicit producer identities.
    [CmdletBinding(DefaultParameterSetName = 'ById')]
    [OutputType([pscustomobject])]
    param(
        [Parameter(Mandatory)] [AllowEmptyCollection()] [object[]] $Artifact,
        [Parameter(Mandatory, ParameterSetName = 'ById')] [long] $ArtifactId,
        [Parameter(Mandatory, ParameterSetName = 'ByIds')] [string] $ArtifactIds,
        [Parameter(Mandatory, ParameterSetName = 'ByName')] [string] $Name,
        [Parameter(Mandatory, ParameterSetName = 'ByPattern')] [string] $Pattern,
        [Parameter(Mandatory, ParameterSetName = 'ByPattern')] [hashtable] $ProducerJob
    )

    $matching = @(switch ($PSCmdlet.ParameterSetName) {
            'ById' { $Artifact | Where-Object { [long] $_.id -eq $ArtifactId } }
            'ByIds' {
                $ids = @($ArtifactIds -split ',' | ForEach-Object {
                        $value = $_.Trim()
                        if ($value -cnotmatch '^[1-9][0-9]*$') {
                            throw "Invalid artifact ID '$value'."
                        }
                        [long] $value
                    })
                if (@($ids | Sort-Object -Unique).Count -ne $ids.Count) {
                    throw 'Artifact IDs must be unique.'
                }
                $idMatches = @($Artifact | Where-Object { [long] $_.id -in $ids })
                $foundIds = @($idMatches | ForEach-Object { [long] $_.id })
                if (@($ids | Where-Object { $_ -notin $foundIds }).Count -gt 0) {
                    return @()
                }
                $idMatches
            }
            'ByName' { $Artifact | Where-Object { [string] $_.name -ceq $Name } }
            'ByPattern' { $Artifact | Where-Object { [string] $_.name -clike $Pattern } }
        })
    if ($PSCmdlet.ParameterSetName -cin @('ById', 'ByIds')) { return @($matching) }

    $latestByName = @(
        $matching |
            Group-Object -Property name -CaseSensitive |
            ForEach-Object { @($_.Group | Sort-Object -Property id -Descending)[0] }
    )
    if ($PSCmdlet.ParameterSetName -ceq 'ByPattern') {
        foreach ($artifactName in $ProducerJob.Keys) {
            if ($artifactName -cnotlike $Pattern) {
                throw "Producer artifact '$artifactName' does not match pattern '$Pattern'."
            }
        }
        return @($latestByName | Where-Object {
                $ProducerJob.ContainsKey([string] $_.name)
            })
    }
    return $latestByName
}

function Assert-GitHubRunArtifactSelection {
    # Rejects wrong-run, expired or malformed metadata before the official action starts.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)] [object[]] $Artifact,
        [Parameter(Mandatory)] [long] $RunId
    )

    foreach ($item in $Artifact) {
        $name = [string] (Get-GitHubArtifactResponseProperty -Response $item -Name 'name')
        $workflowRun = Get-GitHubArtifactResponseProperty -Response $item -Name 'workflow_run'
        $artifactRun = [long] (Get-GitHubArtifactResponseProperty `
                -Response $workflowRun -Name 'id')
        if ($artifactRun -ne $RunId) {
            throw "Artifact '$name' belongs to workflow run $artifactRun, not $RunId."
        }
        if ([bool] (Get-GitHubArtifactResponseProperty -Response $item -Name 'expired')) {
            throw "Artifact '$name' has expired."
        }
    }
}

function Get-GitHubArtifactFreshness {
    # Identifies expected names whose latest metadata predates a producer that actually reran.
    [CmdletBinding()]
    [OutputType([string])]
    param(
        [Parameter(Mandatory)] [object[]] $Artifact,
        [Parameter(Mandatory)] [AllowEmptyCollection()] [object[]] $Job,
        [Parameter(Mandatory)] [hashtable] $ProducerJob,
        [Parameter(Mandatory)] [int] $RunAttempt
    )

    foreach ($artifactName in $ProducerJob.Keys) {
        $latestArtifact = @($Artifact | Where-Object {
                [string] $_.name -ceq $artifactName
            }) | Select-Object -First 1
        if ($null -eq $latestArtifact) {
            $artifactName
            continue
        }

        $currentJobs = @($Job | Where-Object {
                [int] $_.run_attempt -eq $RunAttempt -and
                    (Get-GitHubArtifactJobMarker -Job $_) -ceq $ProducerJob[$artifactName]
            })
        if ($currentJobs.Count -eq 0) { continue }
        if ($currentJobs.Count -ne 1) {
            throw "Expected one current-attempt job named '$($ProducerJob[$artifactName])', " +
                "but found $($currentJobs.Count)."
        }

        $createdAt = [DateTimeOffset]::Parse(
            [string] $latestArtifact.created_at,
            [Globalization.CultureInfo]::InvariantCulture,
            [Globalization.DateTimeStyles]::RoundtripKind
        )
        $startedAt = [DateTimeOffset]::Parse(
            [string] $currentJobs[0].started_at,
            [Globalization.CultureInfo]::InvariantCulture,
            [Globalization.DateTimeStyles]::RoundtripKind
        )
        if ($createdAt -lt $startedAt) { $artifactName }
    }
}

function Wait-GitHubRunArtifact {
    # Waits for a complete, fresh artifact selector through the same inventory used by the official
    # download action. Actual archive download and extraction remain owned by that action.
    [CmdletBinding(DefaultParameterSetName = 'ById')]
    param(
        [Parameter(Mandatory)] [ValidatePattern('^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$')]
        [string] $Repository,
        [Parameter(Mandatory)] [ValidateRange(1, [long]::MaxValue)] [long] $RunId,
        [Parameter(Mandatory)] [uri] $ApiUrl,
        [Parameter(Mandatory)] [ValidateNotNullOrEmpty()] [string] $GitHubToken,
        [Parameter(Mandatory, ParameterSetName = 'ById')]
        [ValidateRange(1, [long]::MaxValue)]
        [long] $ArtifactId,
        [Parameter(Mandatory, ParameterSetName = 'ByIds')]
        [ValidateNotNullOrEmpty()]
        [string] $ArtifactIds,
        [Parameter(Mandatory, ParameterSetName = 'ByName')]
        [ValidateNotNullOrEmpty()]
        [string] $Name,
        [Parameter(Mandatory, ParameterSetName = 'ByPattern')]
        [ValidateNotNullOrEmpty()]
        [string] $Pattern,
        [Parameter(ParameterSetName = 'ByPattern')]
        [ValidateRange(1, [int]::MaxValue)]
        [int] $ExpectedCount = 1,
        [Parameter(Mandatory, ParameterSetName = 'ByPattern')]
        [ValidateNotNullOrEmpty()]
        [hashtable] $ProducerJob,
        [Parameter(Mandatory, ParameterSetName = 'ByPattern')]
        [ValidateRange(1, [int]::MaxValue)]
        [int] $RunAttempt,
        # These defaults cover short propagation without allowing an unbounded CI wait.
        [ValidateRange(1, 100)] [int] $Attempt = $script:MaximumAttempts,
        # A fixed delay avoids tight polling while preserving the bounded retry window.
        [ValidateRange(0, 300)] [int] $DelaySeconds = $script:RetryDelaySeconds,
        # A stalled REST request must fail within the overall handoff budget.
        [ValidateRange(1, 300)] [int] $RequestTimeoutSeconds = $script:RequestTimeoutSeconds
    )

    if ($PSCmdlet.ParameterSetName -ceq 'ByPattern' -and
        $ExpectedCount -ne $ProducerJob.Count) {
        throw 'ExpectedCount must equal the producer-job mapping count.'
    }

    $selector = switch ($PSCmdlet.ParameterSetName) {
        'ById' { "artifact ID $ArtifactId" }
        'ByIds' { "artifact IDs $ArtifactIds" }
        'ByName' { "artifact name '$Name'" }
        'ByPattern' { "artifact pattern '$Pattern'" }
    }
    $headers = @{
        Accept = 'application/vnd.github+json'
        Authorization = "Bearer $GitHubToken"
        'X-GitHub-Api-Version' = '2022-11-28'
    }
    $requestApiUrl = $ApiUrl
    $requestRepository = $Repository
    $requestTimeout = $RequestTimeoutSeconds
    $requestRunAttempt = $RunAttempt

    try {
        Invoke-WithRetry -Attempt $Attempt -DelaySeconds $DelaySeconds -RetryOn {
            param($errorRecord)

            return $errorRecord.FullyQualifiedErrorId -cin @(
                'ArtifactVisibilityPending',
                'ArtifactRequestTransient'
            )
        } -Action {
            $available = @(Get-GitHubRunArtifact -ApiUrl $requestApiUrl `
                    -Repository $requestRepository -RunId $RunId -Headers $headers `
                    -TimeoutSeconds $requestTimeout)
            $selected = @(switch ($PSCmdlet.ParameterSetName) {
                    'ById' {
                        Select-GitHubRunArtifact -Artifact $available -ArtifactId $ArtifactId
                    }
                    'ByIds' {
                        Select-GitHubRunArtifact -Artifact $available -ArtifactIds $ArtifactIds
                    }
                    'ByName' {
                        Select-GitHubRunArtifact -Artifact $available -Name $Name
                    }
                    'ByPattern' {
                        Select-GitHubRunArtifact -Artifact $available -Pattern $Pattern `
                            -ProducerJob $ProducerJob
                    }
                })

            $selectionComplete = if ($PSCmdlet.ParameterSetName -ceq 'ByPattern') {
                $selected.Count -ge $ExpectedCount
            }
            else {
                $selected.Count -gt 0
            }
            if (-not $selectionComplete) {
                throw (Get-GitHubArtifactPendingError `
                        -Message "$selector is not yet visible in workflow run $RunId.")
            }
            Assert-GitHubRunArtifactSelection -Artifact $selected -RunId $RunId

            if ($PSCmdlet.ParameterSetName -ceq 'ByPattern') {
                $jobs = @(Get-GitHubRunAttemptJob -ApiUrl $requestApiUrl `
                        -Repository $requestRepository -RunId $RunId `
                        -RunAttempt $requestRunAttempt -Headers $headers `
                        -TimeoutSeconds $requestTimeout)
                $stale = @(Get-GitHubArtifactFreshness -Artifact $selected -Job $jobs `
                        -ProducerJob $ProducerJob -RunAttempt $requestRunAttempt)
                if ($stale.Count -gt 0) {
                    throw (Get-GitHubArtifactPendingError `
                            -Message "$selector has stale producer artifacts: $($stale -join ', ').")
                }
            }

            Write-Host "$selector is visible and fresh in workflow run $RunId."
            foreach ($artifact in $selected) {
                [pscustomobject]@{
                    Id = [long] (Get-GitHubArtifactResponseProperty -Response $artifact -Name 'id')
                    Name = [string] (Get-GitHubArtifactResponseProperty -Response $artifact -Name 'name')
                }
            }
        }
    }
    catch {
        if ($_.FullyQualifiedErrorId -cin @(
                'ArtifactVisibilityPending',
                'ArtifactRequestTransient'
            )) {
            throw [System.InvalidOperationException]::new(
                "$selector did not become visible after $Attempt attempts in workflow run $RunId."
            )
        }
        throw
    }
}

function Get-GitHubArtifactArchiveStatus {
    # Reads only the REST archive redirect. Redirect following stays disabled so the workflow token
    # never accompanies a request to the signed storage endpoint.
    [CmdletBinding()]
    [OutputType([int])]
    param(
        [Parameter(Mandatory)] [uri] $Uri,
        [Parameter(Mandatory)] [hashtable] $Headers
    )

    $handler = [Net.Http.HttpClientHandler]::new()
    $handler.AllowAutoRedirect = $false
    $client = [Net.Http.HttpClient]::new($handler)
    $client.Timeout = [TimeSpan]::FromSeconds($script:RequestTimeoutSeconds)
    try {
        $client.DefaultRequestHeaders.UserAgent.ParseAdd('folo-artifact-retry')
        foreach ($header in $Headers.GetEnumerator()) {
            $client.DefaultRequestHeaders.Add($header.Key, $header.Value)
        }
        $response = $client.GetAsync($Uri, [Net.Http.HttpCompletionOption]::ResponseHeadersRead).
            GetAwaiter().GetResult()
        try {
            if ([int] $response.StatusCode -eq 302 -and -not $response.Headers.Location) {
                throw 'GitHub returned an archive redirect without a Location.'
            }
            return [int] $response.StatusCode
        }
        finally {
            $response.Dispose()
        }
    }
    finally {
        $client.Dispose()
    }
}

function Wait-GitHubArtifactArchiveRedirect {
    # Probes every frozen ID with the workflow token after an official-action transfer failure.
    # A persistent 404 is only safe to reinterpret for one immutable same-run artifact.
    [CmdletBinding()]
    [OutputType([bool])]
    param(
        [Parameter(Mandatory)] [uri] $ApiUrl,
        [Parameter(Mandatory)] [ValidatePattern('^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$')]
        [string] $Repository,
        [Parameter(Mandatory)] [ValidateRange(1, [long]::MaxValue)] [long[]] $ArtifactIds,
        [Parameter(Mandatory)] [ValidateNotNullOrEmpty()] [string] $GitHubToken,
        [ValidateRange(1, 100)] [int] $Attempt = $script:MaximumAttempts,
        [ValidateRange(0, 300)] [int] $DelaySeconds = $script:RetryDelaySeconds
    )

    $headers = @{
        Accept = 'application/vnd.github+json'
        Authorization = "Bearer $GitHubToken"
        'X-GitHub-Api-Version' = '2022-11-28'
    }
    foreach ($id in $ArtifactIds) {
        $uri = "$($ApiUrl.AbsoluteUri.TrimEnd('/'))/repos/$Repository/actions/artifacts/$id/zip"
        for ($n = 1; $n -le $Attempt; $n++) {
            $status = Get-GitHubArtifactArchiveStatus -Uri $uri -Headers $headers
            if ($status -eq 302) { break }

            $detail = "Archive GET for artifact $id in $Repository returned HTTP $status " +
                "with the workflow token (attempt $n/$Attempt)."
            if ($status -eq 404 -and $n -eq $Attempt -and $ArtifactIds.Count -eq 1) {
                Write-Warning "$detail Retrying the transfer with the same-run artifact service."
                return $false
            }
            if ($status -notin $script:TransientHttpStatuses -or $n -eq $Attempt) {
                throw "$detail Check this job's artifact access; HTTP 404 does not distinguish " +
                    'delay from denial.'
            }
            Write-Warning "$detail Retrying in $DelaySeconds seconds."
            Start-Sleep -Seconds $DelaySeconds
        }
    }
    return $true
}

function Assert-GitHubArtifactTransferComplete {
    # A single artifact either transfers or fails. The official action only warns for a missing
    # member of a multi-ID request, so verify its exact frozen named directory set.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)] [string] $Path,
        [Parameter(Mandatory)] [object[]] $Selection
    )

    if ($Selection.Count -eq 1) { return }

    $missing = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($item in $Selection) {
        if (-not $missing.Add([string] $item.Name)) {
            throw "Cannot distinguish multiple selected artifact IDs named '$($item.Name)'."
        }
    }
    foreach ($entry in (Get-ChildItem -LiteralPath $Path -Force -ErrorAction Stop)) {
        if (-not $entry.PSIsContainer -or -not $missing.Remove($entry.Name)) {
            throw "Incomplete artifact transfer: unexpected staged entry '$($entry.Name)'."
        }
    }
    if ($missing.Count -gt 0) {
        throw "Incomplete artifact transfer: missing $($missing -join ', ')."
    }
}

function Publish-GitHubArtifactTransfer {
    # Exposes only a verified staging tree. Multi-artifact callers may flatten one named artifact
    # directory layer to preserve the official action's merge-multiple layout.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)] [string] $Source,
        [Parameter(Mandatory)] [string] $Destination,
        [Parameter(Mandatory)] [object[]] $Selection,
        [switch] $MergeMultiple
    )

    Assert-GitHubArtifactTransferComplete -Path $Source -Selection $Selection
    if ($Selection.Count -eq 1 -or -not $MergeMultiple) {
        [IO.Directory]::Move($Source, $Destination)
        return
    }

    $null = New-Item -ItemType Directory -Path $Destination -Force
    foreach ($artifact in $Selection) {
        $artifactRoot = Join-Path $Source ([string] $artifact.Name)
        foreach ($entry in (Get-ChildItem -LiteralPath $artifactRoot -Force)) {
            $target = Join-Path $Destination $entry.Name
            if (Test-Path -LiteralPath $target) {
                throw "Cannot expose artifact '$($artifact.Name)' because '$($entry.Name)' already exists."
            }
            Move-Item -LiteralPath $entry.FullName -Destination $target
        }
    }
    Remove-Item -LiteralPath $Source -Recurse -Force
}

Export-ModuleMember -Function `
    Wait-GitHubRunArtifact, `
    Wait-GitHubArtifactArchiveRedirect, `
    Assert-GitHubArtifactTransferComplete, `
    Publish-GitHubArtifactTransfer
