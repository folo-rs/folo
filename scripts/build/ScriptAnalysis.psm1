#requires -Version 7
# Runs the native PowerShell analyzer for validate-scripts without changing its rule set or exit
# semantics. Native error/verbose streams retain the file/rule context of hosted engine failures.
# Includes the shipped Azure deployment driver as well as repository automation.
# Ref: ../../docs/build-and-tooling.md#powershell-linting.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

function Invoke-WorkspaceScriptAnalysis {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $RepositoryRoot,
        [Parameter(Mandatory)][string] $AnalyzerVersion,
        [Parameter(Mandatory)][string] $DiagnosticsDirectory
    )
    Import-Module PSScriptAnalyzer -RequiredVersion $AnalyzerVersion -Force
    $directory = Join-Path $DiagnosticsDirectory ([guid]::NewGuid().ToString('N'))
    $null = New-Item -ItemType Directory -Path $directory -Force
    $trace = Join-Path $directory analyzer-verbose.log
    $paths = @((Join-Path $RepositoryRoot scripts))
    # The source wrapper, canonical driver and native fixture are executable PowerShell too;
    # none loses analysis by living outside scripts/.
    foreach ($relativePath in @(
            'infra\azure-bench-history-prod',
            'infra\azure-bench-history-test',
            'packages\cargo-bench-history\src\azure_bundle',
            'packages\cargo-bench-history\tests\fixtures'
        )) {
        $candidate = Join-Path $RepositoryRoot $relativePath
        if (Test-Path -LiteralPath $candidate -PathType Container) { $paths += $candidate }
    }
    @{
        powershell = $PSVersionTable.PSVersion.ToString()
        edition = $PSVersionTable.PSEdition; platform = [Environment]::OSVersion.ToString()
        culture = [Globalization.CultureInfo]::CurrentCulture.Name
        analyzer = @((Get-Module PSScriptAnalyzer) | Select-Object Name, Version, Path)
        pester = @((Get-Module Pester -ListAvailable) | Select-Object Name, Version, Path)
        module_path = $env:PSModulePath; repository = $RepositoryRoot
        script_paths = $paths
    } | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath (Join-Path $directory environment.json)
    try {
        $customRules = Join-Path $RepositoryRoot 'scripts\analyzer\FoloAnalyzerRules.psm1'
        $rules = @(
            Get-ScriptAnalyzerRule
            Get-ScriptAnalyzerRule -CustomRulePath $customRules
        )
        $names = @($rules | Select-Object -ExpandProperty RuleName | Sort-Object -Unique)
        $customNames = @($rules | Where-Object SourceType -EQ 'Module' | Select-Object -ExpandProperty RuleName)
        if ($names.Count -eq 0) {
            throw 'PSScriptAnalyzer did not discover any rules.'
        }
        # CommandInfo parameter resolution is unsafe across concurrent analyzer rules.
        # Each pass excludes only its peers; the unchanged settings still decide whether
        # its rule runs. No IncludeRule override can accidentally enable an opt-in rule.
        # Ref: ../../docs/build-and-tooling.md#powershell-linting.
        $results = @(foreach ($name in $names) {
            "Rule pass: $name" | Add-Content -LiteralPath $trace
            $peers = @($names | Where-Object { $_ -cne $name })
            # Loading external rules creates a runspace pool for every file even when
            # they are all excluded. Load the custom module only for its own passes.
            $customArguments = @{}
            if ($name -in $customNames) { $customArguments.CustomRulePath = $customRules }
            foreach ($path in $paths) {
                Invoke-ScriptAnalyzer -Path $path -Recurse `
                    -Settings (Join-Path $RepositoryRoot PSScriptAnalyzerSettings.psd1) `
                    -IncludeDefaultRules -ExcludeRule $peers @customArguments -Verbose 4>> $trace
            }
        })
    } catch [System.Management.Automation.RuntimeException], [System.NullReferenceException] {
        # Preserve the original failure. The normal formatter omits managed/inner stacks,
        # while the trace identifies the last files and rules the engine started.
        $_.Exception.ToString() | Set-Content -LiteralPath (Join-Path $directory exception.log)
        @{
            error_id = $_.FullyQualifiedErrorId; category = [string]$_.CategoryInfo
            script_stack = $_.ScriptStackTrace
            position = if ($null -ne $_.InvocationInfo) { $_.InvocationInfo.PositionMessage } else { $null }
        } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $directory error-record.json)
        Write-Host "PSScriptAnalyzer engine failure; diagnostics: $directory"
        Write-Host $_.Exception.ToString()
        throw
    }
    if ($results.Count -gt 0) {
        foreach ($finding in $results) {
            $name = [string]$finding.ScriptName
            if ($name.StartsWith($RepositoryRoot, [StringComparison]::OrdinalIgnoreCase)) {
                $name = $name.Substring($RepositoryRoot.Length).TrimStart('\', '/')
            }
            Write-Host ("{0}:{1} [{2}] {3}: {4}" -f $name, $finding.Line, $finding.Severity, $finding.RuleName, $finding.Message)
        }
        $noun = if ($results.Count -eq 1) { 'issue' } else { 'issues' }
        throw "PSScriptAnalyzer reported $($results.Count) $noun. Fix the findings; no rule was skipped."
    }
    Write-Host "PSScriptAnalyzer: no issues in the configured script inputs. Diagnostics: $directory"
}

Export-ModuleMember -Function Invoke-WorkspaceScriptAnalysis
