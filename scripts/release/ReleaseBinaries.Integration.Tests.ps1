#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Real controller compilation/JSON boundary with the canonical target policy, without GitHub
# queries or publication. Nonempty build/archive fixtures live in release-binaries/tests/smoke/.
BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'ReleaseBinaries.psm1') -Force
    Import-Module (Join-Path $PSScriptRoot 'ReleaseAutomation.psm1') -Force
}

Describe 'Native release planning boundary' {
    It 'accepts every configured runner label and preserves an explicit empty matrix' {
        $plan = @{
            binaries = @()
            targets = @(Get-ReleaseTarget | ForEach-Object { @{ triple = $_.Triple; os = $_.Os } })
        }
        $json = ConvertTo-Json -InputObject $plan -Depth 8 -Compress
        $result = Invoke-ReleaseBinariesHelper -Operation plan -InputJson $json -Repository 'fixture/no-releases'
        $result | Should -Be '[]'
    }

    It 'propagates invalid native input instead of returning an empty plan' {
        { Invoke-ReleaseBinariesHelper -Operation plan -InputJson 'not json' -Repository 'fixture/no-releases' } |
            Should -Throw
    }
}
