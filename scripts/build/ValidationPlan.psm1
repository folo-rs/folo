#requires -Version 7

# Plans non-Cargo Standard validation in the prepare job before toolchain setup, using only
# Git and the runner's PowerShell. The same job adds Cargo dependency impact after setup.
# Ref: .github/workflows/implementation.md#non-cargo-change-planning.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

# Directories are test domains, not independent dependency islands. Shared consumers below
# supplement the owning directory; unknown script/recipe locations select the full suite.
$script:ScriptDomains = @('analyzer', 'bench-history', 'book', 'build', 'release', 'scheduled', 'setup', 'utility')
$script:RecipeDomains = @{
    'just_basics.just' = @('build', 'scheduled')
    'just_bench_history.just' = @('bench-history')
    'just_benchmark_action.just' = @('release')
    'just_book.just' = @('book')
    'just_delta.just' = @('build')
    'just_quality.just' = @('build', 'scheduled')
    'just_quality_mutants.just' = @('build', 'scheduled')
    'just_release.just' = @('release')
}

function Get-ValidationPlan {
    [CmdletBinding()]
    [OutputType([hashtable])]
    param(
        [Parameter(Mandatory)][AllowEmptyCollection()][string[]] $ChangedPath,
        [switch] $Full,
        [bool] $CanaryTrusted = $false
    )

    $domains = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    $workflows = $Full.IsPresent
    $analysis = $Full.IsPresent
    $bicep = $Full.IsPresent
    $canary = $Full.IsPresent
    if ($Full) { $domains.UnionWith([string[]] $script:ScriptDomains) }

    foreach ($path in $ChangedPath) {
        # These inputs define selection or the shared invocation environment. Changes to the
        # planner and fan-in must exercise every selectable check, including their own tests.
        $shared = $path -cin @('justfile', 'constants.env', 'rust-toolchain.toml', '.gitattributes', '.gitconfig', '.gitignore') -or
            $path -cmatch '^scripts/(build/(ValidationPlan|RequiredChecks|Delta)(\.Tests\.ps1|\.psm1)|setup/.+|utility/.+)$' -or
            $path -cmatch '^justfiles/just_(setup|testing)\.just$' -or
            $path -cmatch '^\.github/actions/setup-environment/'
        if ($shared) {
            $workflows = $true
            $analysis = $true
            $bicep = $true
            $canary = $true
            $domains.UnionWith([string[]] $script:ScriptDomains)
            Write-Verbose "'$path' changes shared validation machinery; selecting all tooling checks."
            continue
        }

        if ($path -cmatch '^\.github/fixtures/bench-history-caller/' -or
            $path -cmatch '^scripts/bench-history/' -or
            $path -cmatch '^\.github/actions/bench-history-setup/' -or
            $path -cin @('.github/workflows/standard-validation.yml', '.github/workflows/deep-validation.yml',
                '.github/workflows/benchmark-action-canary.yml', 'justfiles/just_quality.just',
                'delta.toml', '.cargo/config', '.cargo/config.toml')) {
            $canary = $true
            $null = $domains.Add('bench-history')
            Write-Verbose "'$path' affects the synthetic caller or its execution/selection machinery; selecting caller integration."
        }

        if ($path -cmatch '^\.github/workflows/[^/]+\.ya?ml$' -or
            $path -cmatch '^\.github/actions/.+\.ya?ml$' -or
            $path -cin @('.github/actionlint.yaml', '.github/actionlint.yml')) {
            $workflows = $true
            # Pester checks job dependency relationships and the helpers invoked by workflows.
            $domains.UnionWith([string[]] @('build', 'scheduled'))
            Write-Verbose "'$path' is workflow/lint configuration; selecting workflow lint, dependency checks and helper tests."
        }
        if ($path -cmatch '^\.github/skills/scheduled-(intake|triage|repair)/' -or
            $path -cin @('.github/prompts/setup-scheduled-remediation.prompt.md', 'docs/scheduled-validation.md')) {
            $null = $domains.Add('scheduled')
            Write-Verbose "'$path' is an input to documentation-link tests; selecting the scheduled test domain."
        }
        if ($path -cmatch '^infra/azure-bench-history-(prod|test)/' -or
            $path -cmatch '^packages/cargo-bench-history/src/azure_bundle/' -or
            $path -cmatch '^packages/cargo-bench-history/tests/fixtures/.+\.ps(m1|d1|1)$' -or
            $path -cmatch '^\.github/actions/bench-history-setup/' -or
            $path -cin @('.github/workflows/bench-history.yml', '.github/workflows/pr-bench-history.yml', '.github/workflows/bench-history-backfill.yml')) {
            $null = $domains.Add('bench-history')
            if ($path -cmatch '\.ps(m1|d1|1)$') { $analysis = $true }
            Write-Verbose "'$path' owns benchmark deployment or invocation wiring; selecting benchmark helper tests."
        }
        if ($path -ceq 'bicepconfig.json' -or
            $path -cmatch '^(infra/|packages/cargo-bench-history/src/azure_bundle/).+\.bicep(param)?$' -or
            $path -cmatch '^scripts/build/Bicep(\.[^.]+)*\.(psm1|ps1)$') {
            $bicep = $true
            $domains.UnionWith([string[]] @('build', 'scheduled'))
            Write-Verbose "'$path' affects Bicep inputs or their compiler invocation; selecting offline Bicep validation."
        }

        if ($path -ceq 'PSScriptAnalyzerSettings.psd1') {
            $analysis = $true
            $null = $domains.Add('analyzer')
            Write-Verbose "'$path' configures script analysis; selecting analysis and its rule tests."
        }
        if ($path -cmatch '^scripts/') {
            if ($path -cmatch '\.ps(m1|d1|1)$') { $analysis = $true }
            if ($path -cmatch '^scripts/([^/]+)/' -and $Matches[1] -cin $script:ScriptDomains) {
                $null = $domains.Add($Matches[1])
                Write-Verbose "'$path' belongs to script domain '$($Matches[1])'; selecting that domain's tests."
            } else {
                $domains.UnionWith([string[]] $script:ScriptDomains)
                Write-Verbose "'$path' has no registered script domain; conservatively selecting every script suite."
            }
        }
        if ($path -cmatch '^justfiles/(.+)$') {
            $recipe = $Matches[1]
            if ($script:RecipeDomains.ContainsKey($recipe)) {
                $domains.UnionWith([string[]] $script:RecipeDomains[$recipe])
                Write-Verbose "'$path' owns recipes for $($script:RecipeDomains[$recipe] -join ', '); selecting those suites."
            } else {
                $domains.UnionWith([string[]] $script:ScriptDomains)
                Write-Verbose "'$path' has no registered recipe owner; conservatively selecting every script suite."
            }
            # The quality recipe owns both lint commands, including their arguments/settings.
            if ($recipe -ceq 'just_quality.just') { $workflows = $true; $analysis = $true; $bicep = $true }
        }
        if ($path -cin @('delta.toml', '.cargo/mutants.toml', '.config/nextest.toml')) {
            $null = $domains.Add('build')
            Write-Verbose "'$path' configures build/check execution; selecting build-helper tests."
        }
        if ($path -ceq 'release-plz.toml') {
            $null = $domains.Add('release')
            Write-Verbose "'$path' configures release automation; selecting release tests."
        }
        if ($path -ceq '.github/workflows/release.yml' -or
            $path -cmatch '^scripts/build/CargoExecutable\.(psm1|Tests\.ps1)$') {
            $null = $domains.Add('release')
            Write-Verbose "'$path' supplies the release workflow or its native executable boundary; selecting release tests."
        }
        if ($path -cmatch '^\.cargo/config(\.toml)?$') {
            # Cargo fixture tests and real helper builds consume workspace Cargo configuration.
            $domains.UnionWith([string[]] @('release', 'scheduled'))
            Write-Verbose "'$path' affects Cargo fixture and native-helper execution; selecting release and scheduled tests."
        }
        if ($path -ceq 'Cargo.toml' -or $path -cmatch '^packages/[^/]+/Cargo\.toml$') {
            # Scheduled check planning reads live workspace metadata.
            $null = $domains.Add('scheduled')
            Write-Verbose "'$path' is a live metadata input to scheduled integration tests."
        }
    }

    # Scheduled execution imports build helpers. Other cross-domain sharing goes through
    # setup/utility and selects all above.
    if ($domains.Contains('build')) {
        $null = $domains.Add('scheduled')
        Write-Verbose 'Scheduled tests consume build helpers; including that dependent domain.'
    }
    Write-Verbose "Tooling selection: workflows=$workflows, script analysis=$analysis, Bicep=$bicep, script domains=$(@($domains | Sort-Object) -join ', '). Inputs outside declared tooling domains are left to Cargo/package checks."
    return @{
        workflows = $workflows
        script_analysis = $analysis
        bicep = $bicep
        benchmark_canary = $canary
        benchmark_canary_trusted = $CanaryTrusted
        script_domains = @($domains | Sort-Object)
    }
}

function Read-ValidationPlan {
    [CmdletBinding()]
    [OutputType([hashtable])]
    param([Parameter(Mandatory)][AllowEmptyString()][string] $Json)

    $plan = ConvertFrom-Json -InputObject $Json -AsHashtable
    if ($plan -isnot [hashtable] -or $plan.workflows -isnot [bool] -or
        $plan.script_analysis -isnot [bool] -or $plan.bicep -isnot [bool] -or
        $plan.benchmark_canary -isnot [bool] -or $plan.benchmark_canary_trusted -isnot [bool]) {
        throw 'Validation plan must contain explicit tooling and canary scope/trust decisions.'
    }
    $null = Read-ScriptDomain -Value $plan.script_domains
    return $plan
}

function Read-ScriptDomain {
    [CmdletBinding()]
    [OutputType([string])]
    param([Parameter(Mandatory)][AllowNull()][AllowEmptyCollection()][object] $Value)

    if ($Value -isnot [array]) { throw 'Script domains must be an explicit array.' }
    foreach ($domain in $Value) {
        if ($domain -isnot [string] -or $domain -cnotin $script:ScriptDomains) {
            throw "Unknown script test domain '$domain'."
        }
    }
    return @($Value | Sort-Object -Unique)
}

function Get-ValidationScriptDomain {
    # Preparation adds dependency-aware selection for native helpers exercised by Pester.
    # In particular, an unrelated Cargo.lock edit is not a reason to run every script suite.
    [CmdletBinding()]
    [OutputType([string])]
    param(
        [Parameter(Mandatory)][AllowEmptyString()][string] $PlanJson,
        [Parameter(Mandatory)][AllowEmptyString()][string] $AffectedPackageJson
    )

    $plan = Read-ValidationPlan -Json $PlanJson
    $packages = @(Read-ValidationAffectedPackage -Json $AffectedPackageJson)
    $domains = @($plan.script_domains)
    foreach ($package in $packages) {
        if ($package -cin @('cargo-release-plan', 'release-target-check')) {
            $domains += 'release'
            Write-Verbose "Cargo delta selected '$package'; selecting its release verification tests."
        }
    }
    if ((Get-ValidationCanarySelection -PlanJson $PlanJson -AffectedPackageJson $AffectedPackageJson).check_fixture) {
        $domains += 'bench-history'
    }
    return @($domains | Sort-Object -Unique)
}

function Read-ValidationAffectedPackage {
    [CmdletBinding()]
    [OutputType([string])]
    param([Parameter(Mandatory)][AllowEmptyString()][string] $Json)

    $packages = ConvertFrom-Json -InputObject $Json -NoEnumerate
    if ($packages -isnot [array]) { throw 'Affected packages must be an explicit array.' }
    foreach ($package in $packages) {
        if ($package -isnot [string] -or [string]::IsNullOrWhiteSpace($package)) {
            throw 'Affected package names must be nonempty strings.'
        }
    }
    return $packages
}

function Get-ValidationCanarySelection {
    # These are the executable consumers the canary exercises. Cargo delta supplies their
    # transitive dependency impact, including private CBH partitions; do not list those again.
    # Ref: .github/workflows/implementation.md#reusable-workflow-canary.
    [CmdletBinding()]
    [OutputType([hashtable])]
    param(
        [Parameter(Mandatory)][AllowEmptyString()][string] $PlanJson,
        [Parameter(Mandatory)][AllowEmptyString()][string] $AffectedPackageJson
    )

    $plan = Read-ValidationPlan -Json $PlanJson
    $packages = @(Read-ValidationAffectedPackage -Json $AffectedPackageJson)
    $affected = @($packages | Where-Object {
            $_ -cin @('cargo-bench-history', 'cargo-bench-history-github', 'cargo-bench-history-faker')
        })
    $scope = $plan.benchmark_canary -or $affected.Count -gt 0
    Write-Verbose "Caller integration: path/full selection=$($plan.benchmark_canary), affected consumers=$($affected -join ', '), trusted event=$($plan.benchmark_canary_trusted). Fixture checks need no credentials; hosted collection requires both scope and trust."
    return @{
        check_fixture = $scope
        run_hosted = $scope -and $plan.benchmark_canary_trusted
    }
}

function Get-ScriptTestPath {
    # Local `just test-scripts` defaults to the full tree. Explicit domains must exist and contain
    # tests: a misspelled or obsolete selection must not become a successful empty Pester run.
    [CmdletBinding()]
    [OutputType([string])]
    param(
        [string] $Domains = '',
        [string] $Root = (Join-Path $PSScriptRoot '..')
    )

    if ([string]::IsNullOrWhiteSpace($Domains)) { return $Root }
    $selected = @(Read-ScriptDomain -Value @($Domains -split '\s+' | Where-Object { $_ }))
    foreach ($domain in $selected) {
        $path = Join-Path $Root $domain
        if (-not (Test-Path -LiteralPath $path -PathType Container) -or
            @(Get-ChildItem -LiteralPath $path -Filter '*.Tests.ps1' -Recurse -File).Count -eq 0) {
            throw "Selected script domain '$domain' contains no test suite."
        }
        $path
    }
}

function Invoke-ValidationGit {
    # Keep NUL-delimited Git output intact, including filenames with whitespace/newlines, and
    # propagate native failures. PowerShell's line-oriented native pipeline cannot do that.
    [CmdletBinding()]
    [OutputType([string])]
    param([Parameter(Mandatory)][string[]] $Argument)

    $start = [Diagnostics.ProcessStartInfo]::new('git')
    $start.WorkingDirectory = (Get-Location).ProviderPath
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.StandardOutputEncoding = [Text.Encoding]::UTF8
    foreach ($item in $Argument) { $start.ArgumentList.Add($item) }
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $start
    try {
        $null = $process.Start()
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        $process.WaitForExit()
        $output = $stdout.GetAwaiter().GetResult()
        $errorText = $stderr.GetAwaiter().GetResult()
        if ($process.ExitCode -ne 0) { throw "Git validation-scope lookup failed: $errorText" }
        return $output
    } finally {
        $process.Dispose()
    }
}

function Get-ValidationChangedPath {
    [CmdletBinding()]
    [OutputType([string])]
    param(
        [Parameter(Mandatory)][hashtable] $EventData
    )

    $base = $EventData.pull_request.base.sha
    $head = $EventData.pull_request.head.sha
    foreach ($revision in @($base, $head)) {
        if ($revision -isnot [string] -or $revision -cnotmatch '^[0-9a-f]{40}$') {
            throw 'Change planning requires the event base and head commit SHAs.'
        }
    }
    $base = (Invoke-ValidationGit -Argument @('merge-base', $base, $head)).Trim()
    Write-Verbose "Comparing pull-request commits $base..$head; renames contribute both removed and added paths."
    $output = Invoke-ValidationGit -Argument @('diff', '--no-ext-diff', '--no-renames', '--name-only', '-z', $base, $head, '--')
    return $output.Split([char] 0, [StringSplitOptions]::RemoveEmptyEntries)
}

function Get-ValidationWorkflowPlan {
    [CmdletBinding()]
    [OutputType([hashtable])]
    param(
        [Parameter(Mandatory)][ValidateSet('push', 'pull_request', 'schedule', 'workflow_dispatch')][string] $EventName,
        [Parameter(Mandatory)][hashtable] $EventData,
        [Parameter(Mandatory)][string] $Ref,
        [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string] $Repository
    )

    if ($EventName -cne 'pull_request') {
        if ($Ref -cne 'refs/heads/main') { throw 'Full validation is reserved for main.' }
        Write-Verbose "$EventName on main selects every tooling check without a changed-path comparison."
        return Get-ValidationPlan -ChangedPath @() -Full -CanaryTrusted ($Repository -ceq 'folo-rs/folo')
    }
    $headRepository = $EventData.pull_request.head.repo.full_name
    if ($headRepository -isnot [string] -or [string]::IsNullOrWhiteSpace($headRepository)) {
        throw 'Canary credential selection requires the pull-request head repository.'
    }
    $trusted = $Repository -ceq 'folo-rs/folo' -and $headRepository -ceq $Repository
    $paths = @(Get-ValidationChangedPath -EventData $EventData)
    return Get-ValidationPlan -ChangedPath $paths -CanaryTrusted $trusted
}

Export-ModuleMember -Function Get-ValidationPlan, Read-ValidationPlan, Read-ScriptDomain,
    Get-ValidationScriptDomain, Get-ValidationCanarySelection, Get-ScriptTestPath, Get-ValidationWorkflowPlan
