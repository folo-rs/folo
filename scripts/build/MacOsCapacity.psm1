#Requires -Version 7.6

# Supplies read-only evidence to RequiredChecks.psm1 and maps verified hosted macOS
# capacity cancellations to dependencies in the calling gate's known job graph.
# PowerShell is already available in these jobs, even when Rust setup did not run.
# Ref: ../../.github/workflows/implementation.md#macos-runner-capacity.
Set-StrictMode -Version Latest

function Test-MacOsCapacityCancellation {
    [CmdletBinding()]
    param([Parameter(Mandatory)][hashtable] $Job)

    foreach ($field in @('status', 'conclusion', 'labels', 'steps', 'runner_id', 'runner_name', 'annotations')) {
        if (-not $Job.ContainsKey($field)) { return $false }
    }
    if ($Job.status -cne 'completed' -or $Job.conclusion -cne 'cancelled' -or
        $Job.steps -isnot [array] -or $Job.steps.Count -ne 0 -or
        $Job.runner_id -ne 0 -or $Job.runner_name -cne '' -or
        $Job.labels.Count -ne 1 -or $Job.labels[0] -cne 'macos-latest') {
        return $false
    }
    $failures = @($Job.annotations | Where-Object { $_.annotation_level -ceq 'failure' })
    return $failures.Count -gt 0 -and @($Job.annotations | Where-Object {
            -not (($_.annotation_level -ceq 'failure' -and
                    $_.message -ceq 'The job was not acquired by Runner of type hosted even after multiple attempts') -or
                ($_.annotation_level -ceq 'notice' -and
                    $_.message -ceq 'Due to capacity constraints, jobs targeting macOS arm64 runners may experience longer queue times.'))
        }).Count -eq 0
}

function Get-LatestCapacityJob {
    [CmdletBinding()]
    param([Parameter(Mandatory)][AllowEmptyCollection()][hashtable[]] $Job)

    foreach ($group in @($Job | Group-Object -Property name)) {
        $attempt = ($group.Group.run_attempt | Measure-Object -Maximum).Maximum
        $latest = @($group.Group | Where-Object { $_.run_attempt -eq $attempt })
        if ($latest.Count -ne 1) {
            throw "Ambiguous executions for validation job '$($group.Name)'."
        }
        $latest[0]
    }
}

function Get-RunnerCapacityJob {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][ValidatePattern('^[\w.-]+/[\w.-]+$')][string] $Repository,
        [Parameter(Mandatory)][ValidateRange(1, [long]::MaxValue)][long] $RunId,
        [Parameter(Mandatory)][ValidateRange(1, [int]::MaxValue)][int] $RunAttempt,
        [Parameter(Mandatory)][ValidatePattern('^[a-f0-9]{40}$')][string] $HeadSha,
        [Parameter(Mandatory)][ValidateSet('required-checks', 'result')]
        [string] $GateName
    )

    $run = gh api "repos/$Repository/actions/runs/$RunId" | ConvertFrom-Json -AsHashtable
    if ($LASTEXITCODE -ne 0) { throw 'Could not read the validation workflow run.' }
    if ($run.id -ne $RunId -or $run.repository.full_name -cne $Repository -or
        $run.head_sha -cne $HeadSha -or $run.run_attempt -ne $RunAttempt) {
        throw 'Runner-capacity evidence does not describe the current workflow head and attempt.'
    }
    $pages = gh api --paginate --slurp `
        "repos/$Repository/actions/runs/$RunId/jobs?filter=all&per_page=30" |
        ConvertFrom-Json -AsHashtable
    if ($LASTEXITCODE -ne 0) { throw 'Could not read all validation job executions.' }
    $jobs = @(Get-LatestCapacityJob -Job @($pages | ForEach-Object { $_.jobs }))
    $gate = @($jobs | Where-Object {
            $_.run_attempt -eq $RunAttempt -and $_.status -ceq 'in_progress' -and
            ($_.name -ceq $GateName -or $_.name.EndsWith(" / $GateName", [StringComparison]::Ordinal))
        })
    if ($gate.Count -ne 1) { throw 'Could not uniquely identify this running validation gate.' }
    $prefix = $gate[0].name.Substring(0, $gate[0].name.Length - $GateName.Length)
    foreach ($job in $jobs) {
        if (-not $job.name.StartsWith($prefix, [StringComparison]::Ordinal)) { continue }
        $name = $job.name.Substring($prefix.Length)
        if ($name -ceq $GateName) { continue }
        $job.annotations = @()
        if ($job.conclusion -ceq 'cancelled' -and 'macos-latest' -cin $job.labels -and
            $job.steps -is [array] -and $job.steps.Count -eq 0 -and $job.runner_id -eq 0) {
            $check = [uri] $job.check_run_url
            $expected = '^/repos/' + [regex]::Escape($Repository) + '/check-runs/[0-9]+$'
            if ($check.Scheme -cne 'https' -or $check.Host -cne 'api.github.com' -or
                $check.AbsolutePath -cnotmatch $expected) {
                throw 'Runner-capacity evidence has an unexpected check-run identity.'
            }
            $endpoint = $check.AbsolutePath.TrimStart('/')
            $identity = gh api $endpoint | ConvertFrom-Json -AsHashtable
            if ($LASTEXITCODE -ne 0) { throw 'Could not verify the cancelled check-run identity.' }
            if ($identity.name -cne $job.name -or $identity.head_sha -cne $HeadSha -or
                $identity.check_suite.id -ne $run.check_suite_id -or $identity.app.id -ne 15368 -or
                $identity.app.slug -cne 'github-actions' -or
                $identity.status -cne 'completed' -or $identity.conclusion -cne 'cancelled') {
                throw 'Runner-allocation annotations do not belong to this cancelled Actions job.'
            }
            $annotations = gh api --paginate --slurp "$endpoint/annotations?per_page=100" |
                ConvertFrom-Json -AsHashtable
            if ($LASTEXITCODE -ne 0) { throw 'Could not read runner-allocation annotations.' }
            $job.annotations = @($annotations | ForEach-Object { $_ })
            if ($job.annotations.Count -ne $identity.output.annotations_count) {
                throw 'Runner-allocation annotations are incomplete.'
            }
        }
        $job.name = $name
        $job
    }
    $current = gh api "repos/$Repository/actions/runs/$RunId" | ConvertFrom-Json -AsHashtable
    if ($LASTEXITCODE -ne 0) { throw 'Could not refresh the workflow attempt after reading capacity evidence.' }
    if ($current.id -ne $RunId -or $current.repository.full_name -cne $Repository -or
        $current.head_sha -cne $HeadSha -or $current.run_attempt -ne $RunAttempt) {
        throw 'The workflow changed while runner-capacity evidence was being read.'
    }
}

function Get-OptionalMacOsDependency {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][ValidateSet('standard', 'queue', 'canary')]
        [string] $Policy,
        [Parameter(Mandatory)][AllowEmptyCollection()][hashtable[]] $Job,
        [switch] $RequireCanaryResult
    )

    $jobs = @{}
    foreach ($item in @(Get-LatestCapacityJob -Job $Job)) { $jobs.Add($item.name, $item) }
    $required = [Collections.Generic.List[string]]::new()
    $maySkip = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    $capacity = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)

    if ($Policy -cin @('standard', 'queue')) {
        $matrix = if ($Policy -ceq 'standard') { 'clippy-dev-docs' } else { 'clippy-dev' }
        $names = @('ubuntu-latest', 'windows-latest', 'macos-latest') |
            ForEach-Object { "$matrix ($_)" }
        $mac = "$matrix (macos-latest)"
        if (@($names | Where-Object { -not $jobs.ContainsKey($_) }).Count -eq 0 -and
            (Test-MacOsCapacityCancellation -Job $jobs[$mac]) -and
            @($names | Where-Object {
                    $jobs[$_].status -cne 'completed' -or
                    ($_ -cne $mac -and $jobs[$_].conclusion -cne 'success')
                }).Count -eq 0 -and
            @($jobs.Keys | Where-Object { $_.StartsWith("$matrix (", [StringComparison]::Ordinal) -and $_ -cnotin $names }).Count -eq 0) {
            $capacity.Add($matrix) | Out-Null
        }
        if ($Policy -ceq 'queue') { return @($capacity) }
        # A failed canary is examined independently below; successful unrelated root jobs
        # are not reclassified or made dependent on matrix capacity.
        $prefix = 'benchmark-canary / '
        $canaryJobs = @($Job | Where-Object { $_.name.StartsWith($prefix, [StringComparison]::Ordinal) } |
            ForEach-Object { $copy = $_.Clone(); $copy.name = $copy.name.Substring($prefix.Length); $copy })
        if ($canaryJobs.Count -gt 0 -and
            @(Get-OptionalMacOsDependency -Policy canary -Job $canaryJobs -RequireCanaryResult).Count -gt 0) {
            $capacity.Add('benchmark-canary') | Out-Null
        }
        return @($capacity)
    }

    $exerciseMac = 'exercise / cbh-collect:reusable-workflow-canary:macos-latest'
    $backfillMac = 'backfill / backfill (macos-latest)'
    $exerciseAbsent = $jobs.ContainsKey($exerciseMac) -and
        (Test-MacOsCapacityCancellation -Job $jobs[$exerciseMac])
    $backfillAbsent = $jobs.ContainsKey($backfillMac) -and
        (Test-MacOsCapacityCancellation -Job $jobs[$backfillMac])

    $required.Add('exercise / prepare')
    $required.Add('backfill / prepare')
    foreach ($platform in @('ubuntu-latest', 'windows-latest', 'macos-latest')) {
        $required.Add("exercise / cbh-collect:reusable-workflow-canary:$platform")
        $required.Add("backfill / backfill ($platform)")
    }
    if ($exerciseAbsent) { $capacity.Add($exerciseMac) | Out-Null }
    if ($backfillAbsent) { $capacity.Add($backfillMac) | Out-Null }
    if ($Policy -ceq 'canary') {
        foreach ($name in @(
                'configuration', 'storage', 'exercise / analyze', 'backfill / prepare',
                'rolling-backfill / prepare', 'rolling-backfill / backfill (ubuntu-latest)',
                'no-eligible-backfill / prepare', 'verify-rolling-backfill', 'verify-no-work',
                'verify-backfill', 'verify')) {
            if (-not $required.Contains($name)) { $required.Add($name) }
        }
        foreach ($name in @('exercise / preflight', 'exercise / empty-scope',
                'exercise / failed', 'no-eligible-backfill / backfill')) {
            # GitHub can leave this no-work matrix unexpanded; its prepare and
            # verify-no-work jobs must still succeed.
            if ($name -ceq 'no-eligible-backfill / backfill' -and -not $jobs.ContainsKey($name)) {
                continue
            }
            $required.Add($name)
            $maySkip.Add($name) | Out-Null
        }
        if ($exerciseAbsent) {
            $maySkip.Add('exercise / analyze') | Out-Null
            $maySkip.Add('verify') | Out-Null
        }
        if ($backfillAbsent) { $maySkip.Add('verify-backfill') | Out-Null }
        if ($RequireCanaryResult -or $jobs.ContainsKey('result')) { $required.Add('result') }
    }

    foreach ($name in $required) {
        if (-not $jobs.ContainsKey($name)) { return @() }
        $jobResult = $jobs[$name]
        if ($jobResult.status -cne 'completed') { return @() }
        if ($jobResult.conclusion -ceq 'success' -or $capacity.Contains($name)) { continue }
        if ($jobResult.conclusion -ceq 'skipped' -and $maySkip.Contains($name) -and
            $jobResult.steps -is [array] -and $jobResult.steps.Count -eq 0) { continue }
        return @()
    }
    if ($Policy -ceq 'canary') {
        foreach ($name in $jobs.Keys) {
            if (-not $required.Contains($name)) { return @() }
        }
    }
    if ($exerciseAbsent) { 'exercise'; 'verify' }
    if ($backfillAbsent) { 'backfill'; 'verify-backfill' }
}

Export-ModuleMember -Function Get-RunnerCapacityJob, Get-OptionalMacOsDependency
