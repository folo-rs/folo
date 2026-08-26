#requires -Version 7

# Fan-in classification for the Validation `required-checks` job.
#
# GitHub's required-checks field is a string match on the check name and cannot express
# "this matrix job, but only the legs that actually ran". The ruleset therefore requires only
# the literal check `required-checks`. That job needs every merge-blocking Validation job and
# succeeds when each dependency result is `success` or `skipped`. Parsing `toJSON(needs)` is
# the whole job; it lives here so the classification is Pester-tested and visible to
# `just validate-scripts` rather than sitting inline in YAML. See .github/workflows/design.md
# ("Required checks fan-in") and .github/workflows/AGENTS.md.

Set-StrictMode -Version Latest

function Get-RequiredCheckFailure {
    # Returns one `job=result` string per merge-blocking dependency whose result is neither
    # `success` nor `skipped`. Empty when every dependency is an allowed result. A missing
    # `result` property is treated as a failure so a malformed `needs` payload cannot pass the
    # fan-in. Pure, so the allowed-result set is test-covered without GitHub Actions.
    [CmdletBinding()]
    [OutputType([string[]])]
    param(
        [Parameter(Mandatory)][AllowEmptyString()][string] $NeedsJson
    )

    if ([string]::IsNullOrWhiteSpace($NeedsJson)) {
        throw "NEEDS_JSON is empty; the required-checks job cannot classify merge-blocking results."
    }

    $needs = $NeedsJson | ConvertFrom-Json
    if ($null -eq $needs) {
        throw "NEEDS_JSON did not parse as an object; the required-checks job cannot classify merge-blocking results."
    }

    $failure = [System.Collections.Generic.List[string]]::new()
    foreach ($job in $needs.PSObject.Properties) {
        $result = ''
        if ($null -ne $job.Value -and
            ($job.Value.PSObject.Properties.Name -contains 'result') -and
            $null -ne $job.Value.result) {
            $result = [string] $job.Value.result
        }
        if ($result -ne 'success' -and $result -ne 'skipped') {
            if ([string]::IsNullOrWhiteSpace($result)) {
                $result = 'missing'
            }
            $failure.Add("$($job.Name)=$result")
        }
    }
    return @($failure)
}

function Assert-RequiredCheck {
    # Throws when any merge-blocking dependency did not succeed or skip, so the
    # `required-checks` job fails closed. Prints the failing job names for the run log.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][AllowEmptyString()][string] $NeedsJson
    )

    $failure = @(Get-RequiredCheckFailure -NeedsJson $NeedsJson)
    if ($failure.Count -eq 0) {
        Write-Host 'All merge-blocking jobs succeeded or were skipped.'
        return
    }

    $jobNoun = if ($failure.Count -eq 1) { 'job' } else { 'jobs' }
    $listed = $failure -join ', '
    throw "required-checks failed: $($failure.Count) merge-blocking $jobNoun did not succeed or skip ($listed)."
}

Export-ModuleMember -Function Get-RequiredCheckFailure, Assert-RequiredCheck
