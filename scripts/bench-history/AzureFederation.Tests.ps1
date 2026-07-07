#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Pester suite for AzureFederation.psm1. The dotenv parse, the required-constant fail-fast and the
# AZURE_PROD_CLIENT_ID -> AZURE_CLIENT_ID mapping are exercised against fixture files under TestDrive
# (no real constants.env, no real GITHUB_ENV), so the logic the collect/analyze jobs depend on is
# proven without a workflow run.

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'AzureFederation.psm1') -Force
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

Describe 'Set-AzureFederationEnv' {
    BeforeEach {
        $script:ConstantsPath = Join-Path $TestDrive 'constants.env'
        $script:EnvFilePath = Join-Path $TestDrive 'github.env'
        Set-Content -LiteralPath $script:EnvFilePath -Encoding utf8 -Value @()
    }

    It 'maps the prod client id and tenant to the standard AZURE_* names in the env file' {
        Set-Content -LiteralPath $script:ConstantsPath -Encoding utf8 -Value @(
            'AZURE_PROD_CLIENT_ID=client-456'
            'AZURE_TENANT_ID=tenant-123'
            'UNRELATED=ignored'
        )
        Set-AzureFederationEnv -ConstantsPath $script:ConstantsPath -EnvFilePath $script:EnvFilePath | Out-Null

        $written = Get-Content -LiteralPath $script:EnvFilePath
        $written | Should -Contain 'AZURE_CLIENT_ID=client-456'
        $written | Should -Contain 'AZURE_TENANT_ID=tenant-123'
        # The unrelated constant is not leaked into the job env.
        ($written | Where-Object { $_ -like 'UNRELATED=*' }) | Should -BeNullOrEmpty
    }

    It 'returns the exported mapping' {
        Set-Content -LiteralPath $script:ConstantsPath -Encoding utf8 -Value @(
            'AZURE_PROD_CLIENT_ID=client-456'
            'AZURE_TENANT_ID=tenant-123'
        )
        $exported = Set-AzureFederationEnv -ConstantsPath $script:ConstantsPath -EnvFilePath $script:EnvFilePath
        $exported['AZURE_CLIENT_ID'] | Should -Be 'client-456'
        $exported['AZURE_TENANT_ID'] | Should -Be 'tenant-123'
    }

    It 'appends rather than overwriting existing env-file content' {
        Set-Content -LiteralPath $script:EnvFilePath -Encoding utf8 -Value 'PREEXISTING=kept'
        Set-Content -LiteralPath $script:ConstantsPath -Encoding utf8 -Value @(
            'AZURE_PROD_CLIENT_ID=client-456'
            'AZURE_TENANT_ID=tenant-123'
        )
        Set-AzureFederationEnv -ConstantsPath $script:ConstantsPath -EnvFilePath $script:EnvFilePath | Out-Null
        Get-Content -LiteralPath $script:EnvFilePath | Should -Contain 'PREEXISTING=kept'
    }

    It 'fails loudly when a required identifier is missing' {
        Set-Content -LiteralPath $script:ConstantsPath -Encoding utf8 -Value 'AZURE_TENANT_ID=tenant-123'
        { Set-AzureFederationEnv -ConstantsPath $script:ConstantsPath -EnvFilePath $script:EnvFilePath } |
            Should -Throw '*AZURE_PROD_CLIENT_ID*'
    }
}
