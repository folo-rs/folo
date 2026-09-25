#Requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Integration tests for bootstrap/canary constants loading, using fixture files without
# reading real repository configuration or accessing the network.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'Constants.psm1') -Force
}

Describe 'Read-DotEnvFile' {
    It 'parses KEY=value pairs, skipping comments and blank lines' {
        $path = Join-Path $TestDrive 'constants.env'
        Set-Content -LiteralPath $path -Encoding utf8 -Value @(
            '# a comment'
            ''
            'AZURE_TENANT_ID=tenant-123'
            '  AZURE_PROD_CLIENT_ID = client-456  '
        )
        $values = Read-DotEnvFile -Path $path
        $values['AZURE_TENANT_ID'] | Should -Be 'tenant-123'
        $values['AZURE_PROD_CLIENT_ID'] | Should -Be 'client-456'
    }

    It 'keeps everything after the first = so values may contain =' {
        $path = Join-Path $TestDrive 'constants.env'
        Set-Content -LiteralPath $path -Encoding utf8 -Value 'CONNECTION=a=b=c'
        (Read-DotEnvFile -Path $path)['CONNECTION'] | Should -Be 'a=b=c'
    }

    It 'lets a later duplicate key win' {
        $path = Join-Path $TestDrive 'constants.env'
        Set-Content -LiteralPath $path -Encoding utf8 -Value @('K=first', 'K=second')
        (Read-DotEnvFile -Path $path)['K'] | Should -Be 'second'
    }

    It 'returns an absent key as $null rather than throwing under strict mode' {
        $path = Join-Path $TestDrive 'constants.env'
        Set-Content -LiteralPath $path -Encoding utf8 -Value 'K=v'
        (Read-DotEnvFile -Path $path)['NOPE'] | Should -BeNullOrEmpty
    }

    It 'throws when the file does not exist' {
        { Read-DotEnvFile -Path (Join-Path $TestDrive 'missing.env') } | Should -Throw '*does not exist*'
    }
}

Describe 'Get-RequiredConstant' {
    It 'returns the value when present' {
        Get-RequiredConstant -Values @{ K = 'v' } -Name 'K' | Should -Be 'v'
    }

    It 'throws, naming the constant, when the key is absent' {
        { Get-RequiredConstant -Values @{ } -Name 'AZURE_TENANT_ID' } | Should -Throw "*AZURE_TENANT_ID*"
    }

    It 'throws when the value is blank' {
        { Get-RequiredConstant -Values @{ AZURE_TENANT_ID = '   ' } -Name 'AZURE_TENANT_ID' } |
            Should -Throw "*AZURE_TENANT_ID*"
    }
}
