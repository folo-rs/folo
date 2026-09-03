#requires -Version 7

# Fan-in classification for the Validation `required-checks` job.
#
# GitHub's required-checks field is a string match on the check name and cannot express
# "this matrix job, but only the legs that actually ran". The ruleset therefore requires only
# this job. Parsing `toJSON(needs)` is the whole job; it lives here so the classification is
# Pester-tested and visible to `just validate-scripts` rather than sitting inline in YAML.
# See .github/workflows/design.md ("Required checks fan-in") and
# .github/workflows/implementation.md.

Set-StrictMode -Version Latest

function Get-RequiredCheckFailure {
    # Returns one `job=result` string per merge-blocking dependency that is not an allowed
    # result. Jobs named in `$MustSucceedJob` may only be `success`; every other dependency
    # may be `success` or `skipped`. A missing `result` property is treated as a failure so a
    # malformed `needs` payload cannot pass the fan-in. A job named in `$MustSucceedJob` but
    # absent from `needs` is reported as `absent`: this job only observes what `needs` lists,
    # so a name that drifted out of the workflow's `needs:` list would otherwise never be
    # examined and would green the fan-in. An empty needs object is a classification failure,
    # not a vacuously green fan-in. Pure, so the policy is test-covered without GitHub Actions.
    [CmdletBinding()]
    [OutputType([string[]])]
    param(
        [Parameter(Mandatory)][AllowEmptyString()][string] $NeedsJson,
        [Parameter(Mandatory)][AllowEmptyCollection()][string[]] $MustSucceedJob
    )

    if ([string]::IsNullOrWhiteSpace($NeedsJson)) {
        throw "NEEDS_JSON is empty; the required-checks job cannot classify merge-blocking results."
    }

    $needs = $NeedsJson | ConvertFrom-Json
    if ($null -eq $needs) {
        throw "NEEDS_JSON did not parse as an object; the required-checks job cannot classify merge-blocking results."
    }
    if ($needs -is [System.Array]) {
        throw "NEEDS_JSON parsed as an array; the required-checks job cannot classify merge-blocking results."
    }
    if (@($needs.PSObject.Properties).Count -eq 0) {
        throw "NEEDS_JSON has no jobs; the required-checks job cannot classify merge-blocking results."
    }

    $mustSucceed = [System.Collections.Generic.HashSet[string]]::new(
        [string[]] $MustSucceedJob,
        [System.StringComparer]::Ordinal)

    $failure = [System.Collections.Generic.List[string]]::new()
    foreach ($job in $needs.PSObject.Properties) {
        $result = ''
        if ($null -ne $job.Value -and
            ($job.Value.PSObject.Properties.Name -contains 'result') -and
            $null -ne $job.Value.result) {
            $result = [string] $job.Value.result
        }
        if ([string]::IsNullOrWhiteSpace($result)) {
            $result = 'missing'
        }

        $ok = $result -eq 'success'
        if (-not $ok -and $result -eq 'skipped' -and -not $mustSucceed.Contains($job.Name)) {
            $ok = $true
        }
        if (-not $ok) {
            $failure.Add("$($job.Name)=$result")
        }
    }

    $present = [System.Collections.Generic.HashSet[string]]::new(
        [string[]] @($needs.PSObject.Properties.Name),
        [System.StringComparer]::Ordinal)
    foreach ($name in @($mustSucceed | Sort-Object)) {
        if (-not $present.Contains($name)) {
            $failure.Add("$name=absent")
        }
    }
    return @($failure)
}

function Assert-RequiredCheck {
    # Throws when any merge-blocking dependency did not produce an allowed result, so the
    # fan-in fails closed. Prints the failing job names for the run log.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][AllowEmptyString()][string] $NeedsJson,
        [Parameter(Mandatory)][AllowEmptyCollection()][string[]] $MustSucceedJob
    )

    if ($MustSucceedJob.Count -eq 0) {
        throw 'MUST_SUCCEED_JOBS is empty; unconditional merge-blocking jobs cannot be classified.'
    }

    $failure = @(Get-RequiredCheckFailure -NeedsJson $NeedsJson -MustSucceedJob $MustSucceedJob)
    if ($failure.Count -eq 0) {
        Write-Host 'All merge-blocking jobs succeeded or were skipped where skipping is allowed.'
        return
    }

    $listed = $failure -join ', '
    throw "required-checks failed; these merge-blocking jobs did not produce an allowed result: $listed"
}

Export-ModuleMember -Function Assert-RequiredCheck
