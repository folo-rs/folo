#Requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Exercises deploy.ps1's production lifecycle policy through its module with
# in-process Azure CLI responses. Protects additive updates, fresh-container
# ordering, persistent writer retirement and fail-closed CLI handling without
# deploying resources, inspecting copied Bicep text or needing Azure credentials.

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

BeforeDiscovery {
    Import-Module (Join-Path $PSScriptRoot 'ProductionIdentityDeployment.psm1') -Force
}

AfterAll {
    Remove-Module ProductionIdentityDeployment -Force
}

Describe 'Production identity deployment policy' {
    InModuleScope ProductionIdentityDeployment {
        BeforeAll {
            # Shadow the executable even on machines with no Azure CLI. An
            # unexpected unmocked call must fail rather than reach a live account.
            function script:az { throw 'Azure CLI calls must be mocked.' }
        }

        BeforeEach {
            $script:Parameters = @{
                SubscriptionId = 'subscription-one'
                ResourceGroup = 'group-one'
                StorageAccountName = 'historyone'
                ManagedIdentityName = 'writer-one'
                HistoryContainerName = 'history-one'
            }
            $script:Accounts = @(@{ name = 'unrelated' }, @{ name = 'historyone' })
            $script:Containers = @(@{ name = 'unrelated' }, @{ name = 'history-one' })
            $script:WriterExists = $true
            $script:Credentials = [System.Collections.Generic.List[string]]::new()
            $script:Credentials.AddRange([string[]]@(
                    'github-branch-main', 'github-pull-request', 'github-branch-maintenance'
                ))
            $script:DeploymentParameters = @{}
            $script:Deployed = $false
            $script:FailOperation = ''
            $script:FailAfterDeployment = $false
            $script:ReaderClientId = [guid]::NewGuid().ToString()

            Mock az {
                $global:LASTEXITCODE = 0
                $operation = ($args[0..1] -join ' ')
                if ($operation -in @('storage account', 'storage container-rm', 'identity federated-credential', 'deployment group')) {
                    $operation = ($args[0..2] -join ' ')
                }
                if ($operation -eq $script:FailOperation -and
                    (-not $script:FailAfterDeployment -or $script:Deployed)) {
                    $global:LASTEXITCODE = 23
                    return '{}'
                }

                switch ($operation) {
                    'bicep version' { return 'Installed Bicep' }
                    'group create' { return }
                    'storage account list' {
                        return ConvertTo-Json -InputObject $script:Accounts -Compress
                    }
                    'storage container-rm list' {
                        return ConvertTo-Json -InputObject $script:Containers -Compress
                    }
                    'identity list' {
                        $items = @(@{ name = 'unrelated' })
                        if ($script:WriterExists) { $items += @{ name = 'writer-one' } }
                        return ConvertTo-Json -InputObject $items -Compress
                    }
                    'identity federated-credential list' {
                        $items = @($script:Credentials | ForEach-Object { @{ name = $_ } })
                        return ConvertTo-Json -InputObject $items -Compress
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
                        $script:WriterExists = $true
                        if ($script:DeploymentParameters.trustPullRequests -eq 'true' -and
                            -not $script:Credentials.Contains('github-pull-request')) {
                            $script:Credentials.Add('github-pull-request')
                        }
                        $script:Deployed = $true
                        return ConvertTo-Json -InputObject @{
                            readerManagedIdentityClientId = @{ value = $script:ReaderClientId }
                        } -Compress
                    }
                    'identity federated-credential delete' {
                        # Model only the addressed credential; tests verify both
                        # ordering and the complete subscription/identity scope.
                        if (-not $script:Deployed) { throw 'Deletion preceded deployment.' }
                        $name = $args[[array]::IndexOf($args, '--name') + 1]
                        $null = $script:Credentials.Remove($name)
                        return
                    }
                    default { throw "Unexpected Azure CLI operation: $operation" }
                }
            }
        }

        It 'adds a default reader while preserving existing storage and writer trust' {
            $outputs = Invoke-ProductionIdentityDeployment @script:Parameters

            Invoke-ProductionIdentityDeployment @script:Parameters | Out-Null

            $script:DeploymentParameters.createStorageAccount | Should -Be 'false'
            $script:DeploymentParameters.createHistoryContainer | Should -Be 'false'
            $script:DeploymentParameters.trustPullRequests | Should -Be 'true'
            $script:DeploymentParameters.readerManagedIdentityName | Should -Be 'writer-one-reader'
            $script:Credentials | Should -Contain 'github-pull-request'
            $outputs.readerManagedIdentityClientId.value | Should -Be $script:ReaderClientId
            Should -Invoke az -Times 0 -Exactly -ParameterFilter { $args -contains 'delete' }
            Should -Invoke az -Times 2 -Exactly -ParameterFilter {
                $args[0] -eq 'deployment' -and $args -contains 'Incremental'
            }
        }

        It 'bootstraps fresh storage without writer PR trust and preserves that default on repeat deployment' {
            $script:Accounts = @()
            $script:Containers = @()
            $script:WriterExists = $false
            $script:Credentials.Clear()

            Invoke-ProductionIdentityDeployment @script:Parameters | Out-Null

            $script:DeploymentParameters.createStorageAccount | Should -Be 'true'
            $script:DeploymentParameters.createHistoryContainer | Should -Be 'true'
            $script:DeploymentParameters.trustPullRequests | Should -Be 'false'
            $script:Credentials | Should -Not -Contain 'github-pull-request'
            Should -Invoke az -Times 0 -Exactly -ParameterFilter { $args -contains 'container-rm' }
            Should -Invoke az -Times 0 -Exactly -ParameterFilter { $args -contains 'federated-credential' }

            Invoke-ProductionIdentityDeployment @script:Parameters | Out-Null

            $script:DeploymentParameters.createStorageAccount | Should -Be 'false'
            $script:DeploymentParameters.createHistoryContainer | Should -Be 'false'
            $script:DeploymentParameters.trustPullRequests | Should -Be 'false'
            $script:Credentials | Should -Not -Contain 'github-pull-request'
            Should -Invoke az -Times 0 -Exactly -ParameterFilter { $args -contains 'delete' }
        }

        It 'provisions a missing writer without PR trust even when storage already exists' {
            $script:WriterExists = $false
            $script:Credentials.Clear()

            Invoke-ProductionIdentityDeployment @script:Parameters | Out-Null

            $script:DeploymentParameters.createStorageAccount | Should -Be 'false'
            $script:DeploymentParameters.createHistoryContainer | Should -Be 'false'
            $script:DeploymentParameters.trustPullRequests | Should -Be 'false'
            $script:Credentials | Should -Not -Contain 'github-pull-request'
            Should -Invoke az -Times 0 -Exactly -ParameterFilter { $args -contains 'federated-credential' }
        }

        It 'creates a missing container without resetting an existing account or blob service' {
            $script:Containers = @(@{ name = 'other-history' })

            Invoke-ProductionIdentityDeployment @script:Parameters | Out-Null

            $script:DeploymentParameters.createStorageAccount | Should -Be 'false'
            $script:DeploymentParameters.createHistoryContainer | Should -Be 'true'
        }

        It 'preserves absent writer PR trust on an ordinary deployment' {
            $null = $script:Credentials.Remove('github-pull-request')

            Invoke-ProductionIdentityDeployment @script:Parameters | Out-Null

            $script:DeploymentParameters.trustPullRequests | Should -Be 'false'
            $script:Credentials | Should -Not -Contain 'github-pull-request'
            Should -Invoke az -Times 0 -Exactly -ParameterFilter { $args -contains 'delete' }
        }

        It 'retires only the selected writer PR credential and keeps it absent across repeat deployments' {
            Invoke-ProductionIdentityDeployment @script:Parameters -RetireWriterPullRequestTrust | Out-Null
            $script:DeploymentParameters.trustPullRequests | Should -Be 'false'
            $script:Credentials | Should -Not -Contain 'github-pull-request'
            $script:Credentials | Should -Contain 'github-branch-main'
            $script:Credentials | Should -Contain 'github-branch-maintenance'

            Invoke-ProductionIdentityDeployment @script:Parameters | Out-Null
            Invoke-ProductionIdentityDeployment @script:Parameters -RetireWriterPullRequestTrust | Out-Null

            $script:DeploymentParameters.trustPullRequests | Should -Be 'false'
            $script:Credentials | Should -Not -Contain 'github-pull-request'
            Should -Invoke az -Times 1 -Exactly -ParameterFilter {
                ($args[0..2] -join ' ') -eq 'identity federated-credential delete' -and
                $args[[array]::IndexOf($args, '--subscription') + 1] -eq 'subscription-one' -and
                $args[[array]::IndexOf($args, '--resource-group') + 1] -eq 'group-one' -and
                $args[[array]::IndexOf($args, '--identity-name') + 1] -eq 'writer-one' -and
                $args[[array]::IndexOf($args, '--name') + 1] -eq 'github-pull-request' -and
                $args -contains '--yes'
            }
        }

        It 'can bootstrap a writer without PR trust when retirement is explicitly requested' {
            $script:WriterExists = $false
            $script:Credentials.Clear()

            Invoke-ProductionIdentityDeployment @script:Parameters -RetireWriterPullRequestTrust | Out-Null

            $script:DeploymentParameters.trustPullRequests | Should -Be 'false'
            Should -Invoke az -Times 0 -Exactly -ParameterFilter { $args -contains 'delete' }
        }

        It 'forwards custom reader, repository and local-access parameters without widening storage scope' {
            $script:Parameters.ReaderManagedIdentityName = 'custom-reader'
            $script:Parameters.HistoryContainerName = 'custom-history'
            $script:Parameters.GithubOrg = 'owner-one'
            $script:Parameters.GithubRepo = 'repo-one'
            $script:Parameters.LocalPrincipalId = 'local-object-id'
            $script:Parameters.LocalPrincipalType = 'Group'
            $script:Parameters.Location = 'westeurope'

            Invoke-ProductionIdentityDeployment @script:Parameters | Out-Null

            $script:DeploymentParameters.readerManagedIdentityName | Should -Be 'custom-reader'
            $script:DeploymentParameters.historyContainerName | Should -Be 'custom-history'
            $script:DeploymentParameters.githubOrg | Should -Be 'owner-one'
            $script:DeploymentParameters.githubRepo | Should -Be 'repo-one'
            $script:DeploymentParameters.localPrincipalId | Should -Be 'local-object-id'
            $script:DeploymentParameters.localPrincipalType | Should -Be 'Group'
            $script:DeploymentParameters.location | Should -Be 'westeurope'
            Should -Invoke az -Times 1 -Exactly -ParameterFilter {
                $args -contains 'container-rm' -and
                $args[[array]::IndexOf($args, '--storage-account') + 1] -eq 'historyone' -and
                $args[[array]::IndexOf($args, '--resource-group') + 1] -eq 'group-one' -and
                $args[[array]::IndexOf($args, '--subscription') + 1] -eq 'subscription-one'
            }
        }

        It 'rejects using the writer identity as the reader before any CLI call' {
            { Invoke-ProductionIdentityDeployment @script:Parameters -ReaderManagedIdentityName 'WRITER-ONE' } |
                Should -Throw
            Should -Invoke az -Times 0 -Exactly
        }

        It 'rejects invalid container names before any CLI call' -ForEach @('ab', 'Upper-case', 'two--hyphens') {
            $script:Parameters.HistoryContainerName = $_
            { Invoke-ProductionIdentityDeployment @script:Parameters } | Should -Throw
            Should -Invoke az -Times 0 -Exactly
        }

        It 'rejects an uppercase storage account before any CLI call' {
            $script:Parameters.StorageAccountName = 'Uppercase'
            { Invoke-ProductionIdentityDeployment @script:Parameters } | Should -Throw
            Should -Invoke az -Times 0 -Exactly
        }

        It 'fails on <operation> errors without deleting writer trust' -ForEach @(
            @{ operation = 'bicep version' }
            @{ operation = 'group create' }
            @{ operation = 'storage account list' }
            @{ operation = 'storage container-rm list' }
            @{ operation = 'identity list' }
            @{ operation = 'identity federated-credential list' }
            @{ operation = 'deployment group create' }
        ) {
            $script:FailOperation = $operation

            { Invoke-ProductionIdentityDeployment @script:Parameters -RetireWriterPullRequestTrust } |
                Should -Throw

            $script:Credentials | Should -Contain 'github-pull-request'
            $script:Deployed | Should -BeFalse
            Should -Invoke az -Times 0 -Exactly -ParameterFilter { $args -contains 'delete' }
            if ($operation -eq 'bicep version') {
                Should -Invoke az -Times 0 -Exactly -ParameterFilter {
                    ($args[0..1] -join ' ') -eq 'group create'
                }
            }
        }

        It 'does not treat a failed retirement probe as an absent credential' {
            $script:FailOperation = 'identity federated-credential list'
            $script:FailAfterDeployment = $true

            { Invoke-ProductionIdentityDeployment @script:Parameters -RetireWriterPullRequestTrust } |
                Should -Throw

            $script:Deployed | Should -BeTrue
            $script:Credentials | Should -Contain 'github-pull-request'
            Should -Invoke az -Times 0 -Exactly -ParameterFilter { $args -contains 'delete' }
        }

        It 'surfaces failed deletion rather than claiming retirement succeeded' {
            $script:FailOperation = 'identity federated-credential delete'

            { Invoke-ProductionIdentityDeployment @script:Parameters -RetireWriterPullRequestTrust } |
                Should -Throw

            $script:Deployed | Should -BeTrue
            $script:Credentials | Should -Contain 'github-pull-request'
        }
    }
}
