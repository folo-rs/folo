#Requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Guards the throwaway test stack's use of shared Azure preflight without replacing
# its resource lifecycle or granting production workflow access. All Azure calls
# and context resolution stay in-process fakes.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

BeforeAll {
    $script:Wrapper = Join-Path $PSScriptRoot '..' '..' 'infra' 'azure-bench-history-test' 'deploy.ps1'
    Import-Module (Join-Path $PSScriptRoot '..' '..' 'packages' 'cargo-bench-history' 'src' 'azure_bundle' 'ProductionIdentityDeployment.psm1') -Force
    function az { throw 'Azure calls must be mocked.' }
}

Describe 'Test deployment shared preflight' {
    BeforeEach {
        $script:Calls = [System.Collections.Generic.List[string]]::new()
        $script:PreviousEnvironment = @{}
        foreach ($name in @('AZURE_STORAGE_ACCOUNT_NAME', 'AZURE_LOCATION', 'AZURE_MANAGED_IDENTITY_NAME',
                'GITHUB_ORG', 'GITHUB_REPO', 'AZURE_CUSTOM_PRINCIPAL_ID', 'AZURE_CUSTOM_PRINCIPAL_TYPE')) {
            $script:PreviousEnvironment[$name] = [Environment]::GetEnvironmentVariable($name)
        }
        $calls = $script:Calls
        Mock Import-Module {}
        Mock Get-AzureDeploymentContext ({
            $calls.Add('preflight')
            return [pscustomobject]@{ CurrentUserPrincipalId = 'resolved-user' }
        }.GetNewClosure())
        Mock Write-Host {}
        Mock az ({
            $calls.Add($args[0..1] -join ' ')
            return ConvertTo-Json @{
                storageAccountName = @{ value = 'historytests' }
                managedIdentityClientId = @{ value = 'test-client' }
                tenantId = @{ value = 'tenant-one' }
                subscriptionId = @{ value = 'subscription-one' }
                blobEndpoint = @{ value = 'https://historytests.blob.core.windows.net/' }
            }
        }.GetNewClosure())
    }

    AfterEach {
        foreach ($entry in $script:PreviousEnvironment.GetEnumerator()) {
            [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value)
        }
    }

    It 'resolves the current user before deploying the separate test template' {
        & $script:Wrapper -SubscriptionId subscription-one -StorageAccountName historytests -CurrentUser

        $script:Calls | Should -Be @('preflight', 'group create', 'deployment group')
        Should -Invoke Get-AzureDeploymentContext -Times 1 -Exactly -ParameterFilter {
            $SubscriptionId -eq 'subscription-one' -and $CurrentUser
        }
        $env:AZURE_CUSTOM_PRINCIPAL_ID | Should -Be 'resolved-user'
        $env:AZURE_CUSTOM_PRINCIPAL_TYPE | Should -Be 'User'
        $env:AZURE_MANAGED_IDENTITY_NAME | Should -Be 'id-folo-bench-history-ci'
        Should -Invoke az -Times 1 -Exactly -ParameterFilter {
            $args[0] -eq 'deployment' -and $args -contains 'Incremental' -and
            $args[[array]::IndexOf($args, '--template-file') + 1] -eq
                [IO.Path]::GetFullPath((Join-Path (Split-Path $script:Wrapper -Parent) 'main.bicep'))
        }
        Should -Invoke az -Times 0 -Exactly -ParameterFilter {
            $args -notcontains '--subscription' -or $args -contains 'set'
        }
    }

    It 'passes a literal custom principal without requesting current-user lookup' {
        & $script:Wrapper -SubscriptionId subscription-one -StorageAccountName historytests `
            -CustomPrincipalId 'custom-group' -CustomPrincipalType Group

        $env:AZURE_CUSTOM_PRINCIPAL_ID | Should -Be 'custom-group'
        $env:AZURE_CUSTOM_PRINCIPAL_TYPE | Should -Be 'Group'
        Should -Invoke Get-AzureDeploymentContext -Times 1 -Exactly -ParameterFilter { -not $CurrentUser }
    }

    It 'does not mutate Azure when shared preflight fails' {
        Mock Get-AzureDeploymentContext { throw [InvalidOperationException]::new('preflight-canary') }
        { & $script:Wrapper -SubscriptionId subscription-one -StorageAccountName historytests } |
            Should -Throw -ExceptionType ([InvalidOperationException])
        Should -Invoke az -Times 0 -Exactly
    }

    It 'rejects conflicting access requests before preflight' {
        { & $script:Wrapper -SubscriptionId subscription-one -StorageAccountName historytests `
                -CurrentUser -CustomPrincipalId 'custom-group' -CustomPrincipalType Group } |
            Should -Throw
        Should -Invoke Get-AzureDeploymentContext -Times 0 -Exactly
        Should -Invoke az -Times 0 -Exactly
    }
}
