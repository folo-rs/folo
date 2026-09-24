#Requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Compiles owned local fixtures through the real pinned Bicep CLI. No Azure authentication,
# resource lookup or deployment is performed; output is confined to Pester's TestDrive.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'Bicep.psm1') -Force
}

Describe 'Offline Bicep compiler validation' {
    BeforeEach {
        $script:Repository = Join-Path $TestDrive ([guid]::NewGuid().ToString('N'))
        $script:Infra = Join-Path $script:Repository 'infra'
        $bundle = Join-Path $script:Repository 'packages' 'cargo-bench-history' 'src' 'azure_bundle'
        New-Item -ItemType Directory -Path $script:Infra, $bundle -Force | Out-Null
        $script:Diagnostics = Join-Path $script:Repository 'diagnostics'
        Set-Content -LiteralPath (Join-Path $script:Infra 'main.bicep') -Value @'
param example string
output selected string = example
'@
        Set-Content -LiteralPath (Join-Path $script:Infra 'main.bicepparam') -Value @'
using './main.bicep'
param example = 'fixture'
'@
    }

    It 'compiles a template and its parameter file without deployment' {
        Invoke-BicepValidation -RepositoryRoot $script:Repository -Version $env:BICEP_VERSION `
            -DiagnosticsDirectory $script:Diagnostics
        $parameters = Get-Content -LiteralPath (Join-Path $script:Diagnostics 'infra/main.bicepparam.json') -Raw |
            ConvertFrom-Json
        $parameters.parameters.example.value | Should -Be 'fixture'
        Test-Path -LiteralPath (Join-Path $script:Diagnostics 'infra/main.bicep.json.sarif') |
            Should -BeTrue
    }

    It 'fails on a compiler warning even when Bicep can emit ARM JSON' {
        Set-Content -LiteralPath (Join-Path $script:Infra 'main.bicep') -Value @'
param example string
resource account 'Microsoft.Storage/storageAccounts@1900-01-01' existing = {
  name: example
}
output identifier string = account.id
'@
        { Invoke-BicepValidation -RepositoryRoot $script:Repository -Version $env:BICEP_VERSION `
                -DiagnosticsDirectory $script:Diagnostics } | Should -Throw
    }

    It 'fails on syntax errors and retains machine-readable diagnostics' {
        Set-Content -LiteralPath (Join-Path $script:Infra 'main.bicep') -Value 'not valid bicep !'
        { Invoke-BicepValidation -RepositoryRoot $script:Repository -Version $env:BICEP_VERSION `
                -DiagnosticsDirectory $script:Diagnostics } | Should -Throw
        $diagnostic = Get-Content -LiteralPath (Join-Path $script:Diagnostics 'infra/main.bicep.json.sarif') -Raw |
            ConvertFrom-Json
        @($diagnostic.runs.results | Where-Object level -EQ 'error').Count | Should -BeGreaterThan 0
    }
}
