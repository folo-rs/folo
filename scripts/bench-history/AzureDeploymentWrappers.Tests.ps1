#Requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Executes both deployment wrappers with a mocked Cargo boundary. Their distinct
# placement defaults and optional grants must use the same source-built setup-azure
# command, without separate Azure calls or deployment-template paths.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

Describe '<Name> deployment CLI handoff' -ForEach @(
    @{
        Name = 'Production'
        Directory = 'azure-bench-history-prod'
        ResourceGroup = 'folohistory'
        Account = 'folohistory'
        Identity = 'id-folo-bench-history-prod'
        RequiredParameters = @{ SubscriptionId = 'subscription-canary' }
    }
    @{
        Name = 'Test'
        Directory = 'azure-bench-history-test'
        ResourceGroup = 'rg-folo-bench-history'
        Account = 'testhistory'
        Identity = 'id-folo-bench-history-ci'
        RequiredParameters = @{
            SubscriptionId = 'subscription-canary'
            StorageAccountName = 'testhistory'
        }
    }
) {
    BeforeAll {
        $script:Wrapper = Join-Path $PSScriptRoot '..' '..' 'infra' $Directory 'deploy.ps1'
        function cargo { throw 'Cargo calls must be mocked.' }
        function az { throw 'Deployment wrappers must not invoke Azure directly.' }
    }

    BeforeEach {
        $script:State = @{ CargoArgs = @() }
        $state = $script:State
        Mock cargo ({ $state.CargoArgs = @($args) }.GetNewClosure())
        Mock az { throw 'Deployment wrappers must not invoke Azure directly.' }
    }

    AfterEach {
        Should -Invoke az -Times 0 -Exactly
    }

    It 'runs setup-azure from the workspace with Folo defaults' {
        & $script:Wrapper @RequiredParameters

        $script:State.CargoArgs[0] | Should -Be 'run'
        $script:State.CargoArgs | Should -Contain '--locked'
        $manifest = $script:State.CargoArgs[[array]::IndexOf($script:State.CargoArgs, '--manifest-path') + 1]
        [IO.Path]::GetFullPath($manifest) | Should -Be (
            [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..' '..' 'Cargo.toml'))
        )
        $script:State.CargoArgs[[array]::IndexOf($script:State.CargoArgs, '--package') + 1] |
            Should -Be 'cargo-bench-history'
        $script:State.CargoArgs[[array]::IndexOf($script:State.CargoArgs, '--bin') + 1] |
            Should -Be 'cargo-bench-history'
        $separator = [array]::IndexOf($script:State.CargoArgs, '--')
        $script:State.CargoArgs[($separator + 1)..($script:State.CargoArgs.Count - 1)] | Should -Be @(
            'setup-azure', '--subscription-id', 'subscription-canary',
            '--resource-group', $ResourceGroup, '--location', 'swedencentral',
            '--storage-account', $Account, '--managed-identity', $Identity,
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
            CustomPrincipalId = 'principal-canary'
            CustomPrincipalType = $Type
        }
        & $script:Wrapper @parameters

        $separator = [array]::IndexOf($script:State.CargoArgs, '--')
        $script:State.CargoArgs[($separator + 1)..($script:State.CargoArgs.Count - 1)] | Should -Be @(
            'setup-azure', '--subscription-id', 'subscription-canary',
            '--resource-group', $parameters.ResourceGroup, '--location', 'westeurope',
            '--storage-account', 'customhistory', '--managed-identity', 'identity-canary',
            '--container', 'custom-history', '--github-owner', 'owner-canary',
            '--github-repository', 'repository-canary', '--history-branch', 'history/branch',
            '--verbose', '--custom-principal-id', 'principal-canary', '--custom-principal-type', $CliType
        )
    }

    It 'preserves an incomplete custom-grant request for CLI validation' {
        & $script:Wrapper @RequiredParameters -CustomPrincipalType User

        $script:State.CargoArgs | Should -Contain '--custom-principal-type'
        $script:State.CargoArgs | Should -Not -Contain '--custom-principal-id'
    }

    It 'forwards the current-user shortcut without performing identity lookup itself' {
        & $script:Wrapper @RequiredParameters -CurrentUser

        $script:State.CargoArgs | Should -Contain '--current-user'
        $script:State.CargoArgs | Should -Not -Contain '--custom-principal-id'
        $script:State.CargoArgs | Should -Not -Contain '--custom-principal-type'
    }

    It 'propagates CLI failures' {
        Mock cargo { throw [InvalidOperationException]::new('cli-canary') }

        { & $script:Wrapper @RequiredParameters } |
            Should -Throw -ExceptionType ([InvalidOperationException])
    }
}
