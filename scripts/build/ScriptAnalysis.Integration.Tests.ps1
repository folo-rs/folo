#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }
# Integration coverage for validate-scripts: real rule execution and persisted failure diagnostics.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    Import-Module PSScriptAnalyzer -RequiredVersion 1.25.0
    Import-Module (Join-Path $PSScriptRoot 'ScriptAnalysis.psm1')
}

Describe 'Workspace script analysis with the real engine' {
    BeforeEach {
        $script:root = Join-Path $TestDrive ([guid]::NewGuid().ToString('N'))
        $script:diagnostics = Join-Path $root diagnostics
        $null = New-Item -ItemType Directory -Path (Join-Path $root 'scripts\analyzer') -Force
        $null = New-Item -ItemType Directory -Path (Join-Path $root 'scripts\nested') -Force
        Copy-Item -LiteralPath (Join-Path $PSScriptRoot '..\analyzer\FoloAnalyzerRules.psm1') `
            -Destination (Join-Path $root 'scripts\analyzer\FoloAnalyzerRules.psm1')
        # The exported advanced function exercises ResolveParameter; alias and custom-rule
        # findings prove that serial passes do not merely avoid the failing built-in rule.
        @'
function Get-Thing? {
    [CmdletBinding()]
    param()
    gci -Path .
}
Export-ModuleMember -Function Get-Thing?
$Target = @(1, 2)
foreach ($target in $Target) { $target }
'@ | Set-Content -LiteralPath (Join-Path $root 'scripts\nested\exports.psm1')
    }

    It 'retains each configured built-in/custom finding exactly once with comment help <HelpEnabled>' -ForEach @(
        @{ HelpEnabled = $true }, @{ HelpEnabled = $false }
    ) {
        # Opt-in settings and custom-rule wildcards must remain authoritative even when
        # discovery returns other rules. Information is enabled to cover both export consumers.
        @"
@{
    Severity = @('Information', 'Warning', 'Error')
    IncludeRules = @('PSReservedCmdletChar', 'PSProvideCommentHelp', 'PSAvoidUsingCmdletAliases',
        'Measure-*')
    Rules = @{ PSProvideCommentHelp = @{ Enable = `$$HelpEnabled } }
}
"@ | Set-Content -LiteralPath (Join-Path $root PSScriptAnalyzerSettings.psd1)
        $output = @(& {
            try { Invoke-WorkspaceScriptAnalysis $root 1.25.0 $diagnostics } catch { $_ }
        } 6>&1)
        @($output | Where-Object { $_ -is [Management.Automation.ErrorRecord] }).Count | Should -Be 1
        $messages = @($output | Where-Object { $_ -is [Management.Automation.InformationRecord] } |
            ForEach-Object ToString)
        @($messages | Where-Object { $_ -match '\[Warning\] PSReservedCmdletChar:' }).Count | Should -Be 1
        @($messages | Where-Object { $_ -match '\[Warning\] PSAvoidUsingCmdletAliases:' }).Count | Should -Be 1
        @($messages | Where-Object { $_ -match '\[Error\] FoloAvoidForeachVariableShadowsSource:' }).Count | Should -Be 1
        @($messages | Where-Object { $_ -match '\[Information\] PSProvideCommentHelp:' }).Count |
            Should -Be ([int]$HelpEnabled)
        $directory = @(Get-ChildItem -LiteralPath $diagnostics -Directory)[0].FullName
        Test-Path -LiteralPath (Join-Path $directory exception.log) | Should -BeFalse
    }

    It 'honors severity and exclusions instead of enabling every discovered rule' {
        @'
@{
    Severity = @('Warning')
    IncludeRules = @('PSReservedCmdletChar', 'PSProvideCommentHelp', 'PSAvoidUsingCmdletAliases')
    ExcludeRules = @('PSReservedCmdletChar', 'PSAvoidUsingCmdletAliases')
}
'@ | Set-Content -LiteralPath (Join-Path $root PSScriptAnalyzerSettings.psd1)
        { Invoke-WorkspaceScriptAnalysis $root 1.25.0 $diagnostics } | Should -Not -Throw
    }

    It 'applies configured rules to script outside the scripts tree <RelativePath>' -ForEach @(
        @{ RelativePath = 'infra\azure-bench-history-prod\wrapper-rule-canary.ps1' },
        @{ RelativePath = 'packages\cargo-bench-history\src\azure_bundle\bundled-rule-canary.ps1' },
        @{ RelativePath = 'packages\cargo-bench-history\tests\fixtures\native-rule-canary.ps1' }
    ) {
        $fixture = Join-Path $root $RelativePath
        $null = New-Item -ItemType Directory -Path (Split-Path -Parent $fixture) -Force
        'gci -Path .' | Set-Content -LiteralPath $fixture
        @'
@{
    Severity = @('Warning')
    IncludeRules = @('PSAvoidUsingCmdletAliases')
}
'@ | Set-Content -LiteralPath (Join-Path $root PSScriptAnalyzerSettings.psd1)
        $output = @(& {
            try { Invoke-WorkspaceScriptAnalysis $root 1.25.0 $diagnostics } catch { $_ }
        } 6>&1)
        @($output | Where-Object { $_ -is [Management.Automation.ErrorRecord] }).Count | Should -Be 1
        $messages = @($output | Where-Object { $_ -is [Management.Automation.InformationRecord] } |
            ForEach-Object ToString)
        @($messages | Where-Object { $_ -match '\[Warning\] PSAvoidUsingCmdletAliases:' }).Count |
            Should -Be 2
        $fileName = [regex]::Escape([IO.Path]::GetFileName($fixture))
        @($messages | Where-Object { $_ -match $fileName }).Count | Should -Be 1
    }
}

Describe 'Real PowerShell command-metadata regression control' {
    It 'resolves both exported-command and dynamic parameters serially off the pipeline thread' {
        $result = & (Join-Path $PSScriptRoot 'tests\CommandMetadataProbe.ps1')
        $result.Concurrent | Should -BeFalse
        $result.Iterations | Should -BeGreaterThan 0
        $result.Failures | Should -BeNullOrEmpty
    }
}

Describe 'Workspace script-analysis diagnostics' {
    BeforeEach {
        $script:root = Join-Path $TestDrive ([guid]::NewGuid().ToString('N'))
        $script:diagnostics = Join-Path $root diagnostics
        Mock Import-Module -ModuleName ScriptAnalysis { }
        Mock Get-ScriptAnalyzerRule -ModuleName ScriptAnalysis -ParameterFilter { -not $CustomRulePath } {
            @{ RuleName = 'FirstRule'; SourceType = 'Builtin' }
            @{ RuleName = 'SecondRule'; SourceType = 'Builtin' }
        }
        Mock Get-ScriptAnalyzerRule -ModuleName ScriptAnalysis -ParameterFilter { $CustomRulePath } {
            @{ RuleName = 'CustomRule'; SourceType = 'Module'; SourceName = 'FoloAnalyzerRules' }
        }
        Mock Invoke-ScriptAnalyzer -ModuleName ScriptAnalysis -ParameterFilter { $Path } {
            Write-Verbose 'Analyzing the complete script tree and its rules.' -Verbose
        }
    }

    It 'keeps recursive default/custom rules enabled and retains runtime and rule context' {
        Invoke-WorkspaceScriptAnalysis $root 1.25.0 $diagnostics
        Should -Invoke Invoke-ScriptAnalyzer -ModuleName ScriptAnalysis -Exactly -Times 3 -ParameterFilter {
            $Path -ceq (Join-Path $root scripts) -and $Recurse -and $IncludeDefaultRules -and
            $Settings -ceq (Join-Path $root PSScriptAnalyzerSettings.psd1) -and
            $ExcludeRule.Count -eq 2 -and -not $IncludeRule
        }
        Should -Invoke Invoke-ScriptAnalyzer -ModuleName ScriptAnalysis -Exactly -Times 1 -ParameterFilter {
            $CustomRulePath -ceq (Join-Path $root 'scripts\analyzer\FoloAnalyzerRules.psm1') -and
            $ExcludeRule -notcontains 'CustomRule'
        }
        Should -Invoke Invoke-ScriptAnalyzer -ModuleName ScriptAnalysis -Exactly -Times 2 -ParameterFilter {
            -not $CustomRulePath
        }
        $directory = @(Get-ChildItem -LiteralPath $diagnostics -Directory)[0].FullName
        $environment = Get-Content -LiteralPath (Join-Path $directory environment.json) -Raw | ConvertFrom-Json
        $environment.powershell | Should -Be $PSVersionTable.PSVersion.ToString()
        $environment.analyzer[0].Name | Should -Be PSScriptAnalyzer
        Get-Content -LiteralPath (Join-Path $directory analyzer-verbose.log) -Raw | Should -Match 'complete script tree'
    }

    It 'fails on ordinary findings instead of converting them to diagnostic-only warnings' -ForEach @(
        @{ Count = 1 }, @{ Count = 2 }
    ) {
        Mock Invoke-ScriptAnalyzer -ModuleName ScriptAnalysis -ParameterFilter { $Path -and $ExcludeRule -notcontains 'FirstRule' } {
            foreach ($index in 1..$Count) {
                @{ ScriptName = if ($index -eq 1) { Join-Path $root 'scripts\bad.ps1' } else { 'other.ps1' }
                    Line = $index; Severity = 'Warning'; RuleName = 'ExampleRule'; Message = 'An actual finding' }
            }
        }
        { Invoke-WorkspaceScriptAnalysis $root 1.25.0 $diagnostics } | Should -Throw
        Should -Invoke Invoke-ScriptAnalyzer -ModuleName ScriptAnalysis -Exactly -Times 3 -ParameterFilter { $Path }
    }

    It 'retains <ExceptionType> and its inner exception plus the last file/rule context without retrying' -ForEach @(
        @{ ExceptionType = [System.NullReferenceException] }
        @{ ExceptionType = [System.InvalidOperationException] }
        @{ ExceptionType = [System.ArgumentException] }
    ) {
        Mock Invoke-ScriptAnalyzer -ModuleName ScriptAnalysis -ParameterFilter { $Path } {
            Write-Verbose 'Analyzing failing.ps1 with ExampleRule.' -Verbose
            throw $ExceptionType::new('analyzer-canary',
                [IO.IOException]::new('inner-canary'))
        }
        { Invoke-WorkspaceScriptAnalysis $root 1.25.0 $diagnostics } | Should -Throw
        $directory = @(Get-ChildItem -LiteralPath $diagnostics -Directory)[0].FullName
        $exception = Get-Content -LiteralPath (Join-Path $directory exception.log) -Raw
        $exception | Should -Match 'analyzer-canary'
        $exception | Should -Match ([regex]::Escape($ExceptionType.FullName))
        $exception | Should -Match 'inner-canary'
        Get-Content -LiteralPath (Join-Path $directory analyzer-verbose.log) -Raw | Should -Match 'failing.ps1'
        Test-Path -LiteralPath (Join-Path $directory error-record.json) | Should -BeTrue
        Should -Invoke Invoke-ScriptAnalyzer -ModuleName ScriptAnalysis -Exactly -Times 1 -ParameterFilter { $Path }
    }

    It 'does not report success when discovery fails or returns no rules' -ForEach @(
        @{ FailDiscovery = $true }, @{ FailDiscovery = $false }
    ) {
        Mock Get-ScriptAnalyzerRule -ModuleName ScriptAnalysis -ParameterFilter { -not $CustomRulePath } {
            if ($FailDiscovery) { throw [InvalidOperationException]::new('discovery canary') }
        }
        Mock Get-ScriptAnalyzerRule -ModuleName ScriptAnalysis -ParameterFilter { $CustomRulePath } {
            if ($FailDiscovery) { throw [InvalidOperationException]::new('discovery canary') }
        }
        { Invoke-WorkspaceScriptAnalysis $root 1.25.0 $diagnostics } | Should -Throw
        Should -Invoke Invoke-ScriptAnalyzer -ModuleName ScriptAnalysis -Exactly -Times 0 -ParameterFilter { $Path }
    }
}
