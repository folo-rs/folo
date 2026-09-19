#Requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Executes the production wrapper with a mocked Cargo boundary, guarding its public CLI handoff.
# Deployment defaults and optional local grants reach setup-azure without invoking Azure.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

BeforeAll {
    $script:Wrapper = Join-Path $PSScriptRoot '..' '..' 'infra' 'azure-bench-history-prod' 'deploy.ps1'
}

Describe 'Production deployment CLI handoff' {
    BeforeEach {
        $script:State = @{ CargoArgs = @() }
        $state = $script:State
        Mock cargo ({ $state.CargoArgs = @($args) }.GetNewClosure())
    }

    It 'runs setup-azure from the workspace with Folo defaults' {
        & $script:Wrapper -SubscriptionId 'subscription-canary'

        $script:State.CargoArgs[0] | Should -Be 'run'
        $script:State.CargoArgs | Should -Contain '--locked'
        $script:State.CargoArgs[[array]::IndexOf($script:State.CargoArgs, '--package') + 1] |
            Should -Be 'cargo-bench-history'
        $script:State.CargoArgs[[array]::IndexOf($script:State.CargoArgs, '--bin') + 1] |
            Should -Be 'cargo-bench-history'
        $separator = [array]::IndexOf($script:State.CargoArgs, '--')
        $script:State.CargoArgs[($separator + 1)..($script:State.CargoArgs.Count - 1)] | Should -Be @(
            'setup-azure', '--subscription-id', 'subscription-canary',
            '--resource-group', 'folohistory', '--location', 'swedencentral',
            '--storage-account', 'folohistory', '--managed-identity', 'id-folo-bench-history-prod',
            '--container', 'bench-history', '--github-owner', 'folo-rs',
            '--github-repository', 'folo', '--history-branch', 'main', '--verbose'
        )
    }

    It 'forwards literal overrides and translates the principal type: <Type>' -ForEach @(
        @{ Type = 'User'; CliType = 'user' }
        @{ Type = 'Group'; CliType = 'group' }
    ) {
        $parameters = @{
            SubscriptionId = 'subscription-canary'
            ResourceGroup = 'group''"$() literal'
            Location = 'westeurope'
            StorageAccountName = 'customhistory'
            ManagedIdentityName = 'identity-canary'
            HistoryContainerName = 'custom-history'
            GithubOrg = 'owner-canary'
            GithubRepo = 'repository-canary'
            HistoryBranch = 'history/branch'
            LocalPrincipalId = 'principal-canary'
            LocalPrincipalType = $Type
        }
        & $script:Wrapper @parameters

        $separator = [array]::IndexOf($script:State.CargoArgs, '--')
        $script:State.CargoArgs[($separator + 1)..($script:State.CargoArgs.Count - 1)] | Should -Be @(
            'setup-azure', '--subscription-id', 'subscription-canary',
            '--resource-group', $parameters.ResourceGroup, '--location', 'westeurope',
            '--storage-account', 'customhistory', '--managed-identity', 'identity-canary',
            '--container', 'custom-history', '--github-owner', 'owner-canary',
            '--github-repository', 'repository-canary', '--history-branch', 'history/branch',
            '--verbose', '--local-principal-id', 'principal-canary', '--local-principal-type', $CliType
        )
    }

    It 'preserves an incomplete local-grant request for CLI validation' {
        & $script:Wrapper -SubscriptionId 'subscription-canary' -LocalPrincipalType User

        $script:State.CargoArgs | Should -Contain '--local-principal-type'
        $script:State.CargoArgs | Should -Not -Contain '--local-principal-id'
    }

    It 'propagates CLI failures' {
        Mock cargo { throw [InvalidOperationException]::new('cli-canary') }

        { & $script:Wrapper -SubscriptionId 'subscription-canary' } |
            Should -Throw -ExceptionType ([InvalidOperationException])
    }
}
