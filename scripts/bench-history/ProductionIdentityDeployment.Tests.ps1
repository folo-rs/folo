#Requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Exercises the canonical exported deployment module with in-process Azure
# responses. Guards single-identity additive provisioning, prerequisite ordering
# and fail-closed management-plane discovery, without credentials or mutations.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

BeforeDiscovery {
    Import-Module (Join-Path $PSScriptRoot '..' '..' 'packages' 'cargo-bench-history' 'src' 'azure_bundle' 'ProductionIdentityDeployment.psm1') -Force
}

AfterAll {
    Remove-Module ProductionIdentityDeployment -Force
}

Describe 'Standalone deployment parameter input' {
    BeforeAll {
        $script:Driver = Join-Path $PSScriptRoot '..' '..' 'packages' 'cargo-bench-history' 'src' 'azure_bundle' 'deploy.ps1'
    }

    BeforeEach {
        $script:Overrides = @{
            ParametersFile = 'edited-parameters.json'
            SubscriptionId = 'subscription-one'
            ResourceGroup = 'group-one'
            Location = 'westeurope'
            StorageAccountName = 'historyone'
            GithubOrg = 'owner-one'
            GithubRepo = 'repository-one'
            HistoryBranch = 'main'
        }
        # Keep this script-input test in-process and prevent reimport from
        # replacing the mocked deployment boundary with an Azure-capable one.
        Mock Import-Module {}
        Mock Invoke-ProductionIdentityDeployment { throw 'deployment-boundary-canary' }
    }

    It 'rejects unknown keys even when null and flags supply required inputs: <json>' -ForEach @(
        @{ json = '{"SubcriptionId":null}' }
        @{ json = '{"Keys":null}' }
    ) {
        Mock Get-Content ({ $json }.GetNewClosure())
        { & $script:Driver @script:Overrides } | Should -Throw
        Should -Invoke Invoke-ProductionIdentityDeployment -Times 0 -Exactly
    }

    It 'rejects a non-object JSON root: <json>' -ForEach @(
        @{ json = '[{}]' }
        @{ json = '[]' }
        @{ json = '"not an object"' }
        @{ json = 'null' }
    ) {
        Mock Get-Content ({ $json }.GetNewClosure())
        { & $script:Driver @script:Overrides } | Should -Throw
        Should -Invoke Invoke-ProductionIdentityDeployment -Times 0 -Exactly
    }

    It 'accepts an object root with required values supplied by explicit flags' {
        Mock Get-Content { '{}' }
        { & $script:Driver @script:Overrides } | Should -Throw '*deployment-boundary-canary*'
        Should -Invoke Invoke-ProductionIdentityDeployment -Times 1 -Exactly
    }
}

Describe 'Production identity deployment policy' {
    InModuleScope ProductionIdentityDeployment {
        BeforeAll {
            function script:az { throw 'Azure CLI calls must be mocked.' }
        }

        BeforeEach {
            $script:Parameters = @{
                SubscriptionId = 'subscription-one'
                ResourceGroup = 'group-one'
                Location = 'westeurope'
                StorageAccountName = 'historyone'
                ManagedIdentityName = 'identity-one'
                HistoryContainerName = 'history-one'
                GithubOrg = 'owner-one'
                GithubRepo = 'repository-one'
                HistoryBranch = 'history/main'
            }
            $script:Accounts = @(@{ name = 'unrelated' }, @{ name = 'historyone' })
            $script:Containers = @(@{ name = 'unrelated' }, @{ name = 'history-one' })
            $script:Calls = [System.Collections.Generic.List[string]]::new()
            $script:DeploymentParameters = @{}
            $script:FailOperation = ''
            $script:FailedStdout = '{}'
            $script:SubscriptionState = 'Enabled'
            $script:ReturnedSubscription = 'subscription-one'
            $script:MissingOutput = $false

            Mock az {
                $global:LASTEXITCODE = 0
                $operation = if ($args[0] -eq 'version') { 'version' } else { $args[0..1] -join ' ' }
                if ($operation -in @('storage account', 'storage container-rm', 'deployment group')) {
                    $operation = $args[0..2] -join ' '
                }
                $script:Calls.Add($operation)
                if ($operation -eq $script:FailOperation) {
                    $global:LASTEXITCODE = 23
                    return $script:FailedStdout
                }
                switch ($operation) {
                    'version' { return '{}' }
                    'bicep version' { return 'Installed Bicep' }
                    'account show' {
                        return ConvertTo-Json -InputObject @{
                            id = $script:ReturnedSubscription
                            state = $script:SubscriptionState
                            tenantId = 'tenant-one'
                        }
                    }
                    'account get-access-token' { return 'expiry-not-token' }
                    'group create' { return }
                    'storage account list' {
                        return ConvertTo-Json -InputObject $script:Accounts -Compress
                    }
                    'storage container-rm list' {
                        return ConvertTo-Json -InputObject $script:Containers -Compress
                    }
                    'deployment group create' {
                        $start = [array]::IndexOf($args, '--parameters') + 1
                        $end = [array]::IndexOf($args, '--query')
                        foreach ($argument in $args[$start..($end - 1)]) {
                            $key, $value = $argument.Split('=', 2)
                            $script:DeploymentParameters[$key] = $value
                        }
                        if ($script:DeploymentParameters.createStorageAccount -eq 'true') {
                            $script:Accounts += @{ name = $script:DeploymentParameters.storageAccountName }
                        }
                        if ($script:DeploymentParameters.createHistoryContainer -eq 'true') {
                            $script:Containers += @{ name = $script:DeploymentParameters.historyContainerName }
                        }
                        $outputs = @{
                            storageAccountName = @{ value = 'historyone' }
                            historyContainerName = @{ value = 'history-one' }
                            blobEndpoint = @{ value = 'https://historyone.blob.core.windows.net/' }
                            managedIdentityClientId = @{ value = 'client-one' }
                            managedIdentityPrincipalId = @{ value = 'principal-one' }
                            tenantId = @{ value = 'tenant-one' }
                            subscriptionId = @{ value = 'subscription-one' }
                        }
                        if ($script:MissingOutput) { $outputs.Remove('managedIdentityPrincipalId') }
                        return ConvertTo-Json -InputObject $outputs -Compress
                    }
                    default { throw "Unexpected Azure CLI operation: $operation" }
                }
            }
        }

        It 'ensures one configured identity and branch on repeated deployments while preserving storage' {
            $outputs = Invoke-ProductionIdentityDeployment @script:Parameters
            Invoke-ProductionIdentityDeployment @script:Parameters | Out-Null

            $script:DeploymentParameters.createStorageAccount | Should -Be 'false'
            $script:DeploymentParameters.createHistoryContainer | Should -Be 'false'
            $script:DeploymentParameters.managedIdentityName | Should -Be 'identity-one'
            $script:DeploymentParameters.historyBranch | Should -Be 'history/main'
            $outputs.managedIdentityClientId.value | Should -Be 'client-one'
            Should -Invoke az -Times 2 -Exactly -ParameterFilter {
                $args[0] -eq 'deployment' -and $args -contains 'Incremental'
            }
            Should -Invoke az -Times 0 -Exactly -ParameterFilter { $args -contains 'delete' }
            Should -Invoke az -Times 0 -Exactly -ParameterFilter { $args[0] -eq 'identity' }
        }

        It 'bootstraps missing storage once and preserves it on repeat deployment' {
            $script:Accounts = @()
            $script:Containers = @()
            Invoke-ProductionIdentityDeployment @script:Parameters | Out-Null
            $script:DeploymentParameters.createStorageAccount | Should -Be 'true'
            $script:DeploymentParameters.createHistoryContainer | Should -Be 'true'
            Should -Invoke az -Times 0 -Exactly -ParameterFilter { $args -contains 'container-rm' }

            Invoke-ProductionIdentityDeployment @script:Parameters | Out-Null
            $script:DeploymentParameters.createStorageAccount | Should -Be 'false'
            $script:DeploymentParameters.createHistoryContainer | Should -Be 'false'
            $script:DeploymentParameters.managedIdentityName | Should -Be 'identity-one'
            $script:DeploymentParameters.historyBranch | Should -Be 'history/main'
        }

        It 'creates only a missing container' {
            $script:Containers = @(@{ name = 'unrelated' })
            Invoke-ProductionIdentityDeployment @script:Parameters | Out-Null
            $script:DeploymentParameters.createStorageAccount | Should -Be 'false'
            $script:DeploymentParameters.createHistoryContainer | Should -Be 'true'
        }

        It 'derives the identity from the selected account when omitted' {
            $script:Parameters.Remove('ManagedIdentityName')
            Invoke-ProductionIdentityDeployment @script:Parameters | Out-Null
            $script:DeploymentParameters.managedIdentityName | Should -Be 'id-historyone-bench-history'
        }

        It 'forwards literal custom parameters and independent local access' {
            $script:Parameters.ResourceGroup = 'group''"$() literal'
            $script:Parameters.LocalPrincipalId = 'local-one'
            $script:Parameters.LocalPrincipalType = 'Group'
            Invoke-ProductionIdentityDeployment @script:Parameters | Out-Null
            $script:DeploymentParameters.localPrincipalId | Should -Be 'local-one'
            $script:DeploymentParameters.localPrincipalType | Should -Be 'Group'
            $script:DeploymentParameters.githubOrg | Should -Be 'owner-one'
            $script:DeploymentParameters.githubRepo | Should -Be 'repository-one'
            Should -Invoke az -Times 1 -Exactly -ParameterFilter {
                $args -contains 'container-rm' -and
                $args[[array]::IndexOf($args, '--resource-group') + 1] -eq 'group''"$() literal'
            }
        }

        It 'checks tooling and usable subscription authentication before resource-group creation' {
            Invoke-ProductionIdentityDeployment @script:Parameters | Out-Null
            $script:Calls[0..4] | Should -Be @(
                'version', 'bicep version', 'account show', 'account get-access-token', 'group create'
            )
            Should -Invoke az -Times 0 -Exactly -ParameterFilter {
                $args[0] -notin @('version', 'bicep') -and
                ($args -notcontains '--subscription' -or
                    $args[[array]::IndexOf($args, '--subscription') + 1] -ne 'subscription-one')
            }
        }

        It 'rejects unusable or mismatched subscription context' -ForEach @('disabled', 'mismatched') {
            if ($_ -eq 'disabled') { $script:SubscriptionState = 'Disabled' }
            else { $script:ReturnedSubscription = 'other-subscription' }
            { Invoke-ProductionIdentityDeployment @script:Parameters } | Should -Throw
            Should -Invoke az -Times 0 -Exactly -ParameterFilter { $args -contains 'create' }
        }

        It 'accepts the selected subscription GUID with different letter casing' {
            $script:Parameters.SubscriptionId = 'ABcdef01-2345-6789-abCD-0123456789Ab'
            $script:ReturnedSubscription = $script:Parameters.SubscriptionId.ToLowerInvariant()
            Invoke-ProductionIdentityDeployment @script:Parameters | Out-Null
            Should -Invoke az -Times 1 -Exactly -ParameterFilter {
                $args[0] -eq 'deployment' -and $args -contains 'create'
            }
        }

        It 'rejects invalid input before Azure calls' -ForEach @(
            @{ name = 'HistoryContainerName'; value = 'Uppercase' }
            @{ name = 'HistoryContainerName'; value = 'two--hyphens' }
            @{ name = 'StorageAccountName'; value = 'Uppercase' }
            @{ name = 'LocalPrincipalId'; value = 'unpaired' }
            @{ name = 'LocalPrincipalType'; value = 'User' }
        ) {
            $script:Parameters[$name] = $value
            { Invoke-ProductionIdentityDeployment @script:Parameters } | Should -Throw
            Should -Invoke az -Times 0 -Exactly
        }

        It 'rejects an invalid literal branch before Azure calls: <_>' -ForEach @(
            'refs/heads/main'
            'refs/tags/release'
            'HEAD'
            '-topic'
            '.hidden'
            'release/.hidden'
            'main.lock'
            'release/topic.lock/next'
            'release/topic.lock'
            'topic..next'
            'topic@{1}'
            '/topic'
            'topic/'
            'release//topic'
            'topic.'
            'topic:next'
            'topic~1'
            'topic^1'
            'topic?'
            'topic*'
            'topic[1]'
            'topic\next'
        ) {
            $script:Parameters.HistoryBranch = $_
            { Invoke-ProductionIdentityDeployment @script:Parameters } | Should -Throw
            Should -Invoke az -Times 0 -Exactly
        }

        It 'rejects ASCII controls, space and DEL before Azure calls' {
            foreach ($code in @(0..32) + 127) {
                $script:Parameters.HistoryBranch = "topic$([char]$code)next"
                { Invoke-ProductionIdentityDeployment @script:Parameters } | Should -Throw
                Should -Invoke az -Times 0 -Exactly
            }
        }

        It 'preserves valid literal branches: <_>' -ForEach @(
            '@'
            'head'
            'release/HEAD'
            'release/-topic'
            'release./next'
            'release/topic.LOCK/next'
            'topic.locked'
            "topic`u{00a0}next"
        ) {
            $script:Parameters.HistoryBranch = $_
            Invoke-ProductionIdentityDeployment @script:Parameters | Out-Null
            $script:DeploymentParameters.historyBranch | Should -BeExactly $_
        }

        It 'fails closed on <operation>' -ForEach @(
            @{ operation = 'version' }
            @{ operation = 'bicep version' }
            @{ operation = 'account show' }
            @{ operation = 'account get-access-token' }
            @{ operation = 'group create' }
            @{ operation = 'storage account list' }
            @{ operation = 'storage container-rm list' }
            @{ operation = 'deployment group create' }
        ) {
            $script:FailOperation = $operation
            { Invoke-ProductionIdentityDeployment @script:Parameters } | Should -Throw
            $script:Calls[-1] | Should -Be $operation
            Should -Invoke az -Times 0 -Exactly -ParameterFilter { $args -contains 'delete' }
        }

        It 'does not claim success for missing deployment outputs' {
            $script:MissingOutput = $true
            { Invoke-ProductionIdentityDeployment @script:Parameters } | Should -Throw
        }

        It 'retains stdout diagnostics from a failed native operation' {
            $script:FailOperation = 'deployment group create'
            $script:FailedStdout = 'operation-detail-canary'
            { Invoke-ProductionIdentityDeployment @script:Parameters } |
                Should -Throw '*operation-detail-canary*'
        }
    }
}
