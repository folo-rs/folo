#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Checks the ARM64 CI runtime against the Azure bundle's executable prerequisites without
# running deployment code. The bench-history domain covers both bootstrap and bundle edits.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeDiscovery {
    $repositoryRoot = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
    $scriptDirectories = @(
        (Join-Path $repositoryRoot 'packages/cargo-bench-history/src/azure_bundle'),
        (Join-Path $repositoryRoot 'packages/cargo-bench-history/tests/fixtures')
    )
    $script:PrerequisiteScripts = @(Get-ChildItem -LiteralPath $scriptDirectories -Recurse -File |
        Where-Object { $_.Extension -in @('.ps1', '.psm1') } |
        ForEach-Object { @{ ScriptPath = $_.FullName; ScriptName = $_.Name } })
}

Describe 'Linux ARM64 PowerShell prerequisite compatibility' {
    BeforeAll {
        $script:RepositoryRoot = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
        $action = Get-Content -LiteralPath (Join-Path $script:RepositoryRoot '.github/actions/setup-environment/action.yml') -Raw
        $pin = [regex]::Match($action, '(?m)^\s*PWSH_VERSION="([^"]+)"\s*$')
        $pin.Success | Should -BeTrue
        $script:InstalledVersion = [version] $pin.Groups[1].Value
    }

    It 'meets the executable prerequisite of <ScriptName>' -ForEach $script:PrerequisiteScripts {
        $parseErrors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile(
            $ScriptPath, [ref] $null, [ref] $parseErrors)
        $parseErrors | Should -BeNullOrEmpty
        $requiredVersion = $ast.ScriptRequirements.RequiredPSVersion
        $requiredVersion | Should -Not -BeNullOrEmpty
        $script:InstalledVersion | Should -BeGreaterOrEqual $requiredVersion
    }
}
