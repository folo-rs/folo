#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Pester suite for RequiredChecks.psm1. The allowed-result set (`success` / `skipped`) is the
# contract the Validation `required-checks` fan-in publishes to GitHub, so it is exercised
# against realistic `toJSON(needs)` payloads here rather than only in CI.

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'RequiredChecks.psm1') -Force
}

Describe 'Get-RequiredCheckFailure' {
    It 'returns nothing when every job succeeded' {
        $json = '{"delta":{"result":"success"},"test-scripts":{"result":"success"}}'
        Get-RequiredCheckFailure -NeedsJson $json | Should -BeNullOrEmpty
    }

    It 'treats skipped as allowed' {
        $json = '{"delta":{"result":"success"},"test-arm":{"result":"skipped"}}'
        Get-RequiredCheckFailure -NeedsJson $json | Should -BeNullOrEmpty
    }

    It 'reports a failed job as job=result' {
        $json = '{"delta":{"result":"success"},"clippy-dev":{"result":"failure"}}'
        Get-RequiredCheckFailure -NeedsJson $json | Should -Be @('clippy-dev=failure')
    }

    It 'reports cancelled and other non-allowed results' {
        $json = '{"mutants":{"result":"cancelled"},"hack":{"result":"neutral"}}'
        $result = @(Get-RequiredCheckFailure -NeedsJson $json)
        $result | Should -Contain 'mutants=cancelled'
        $result | Should -Contain 'hack=neutral'
    }

    It 'treats a missing result property as a failure' {
        $json = '{"delta":{"outputs":{}}}'
        Get-RequiredCheckFailure -NeedsJson $json | Should -Be @('delta=missing')
    }

    It 'throws when the payload is empty' {
        { Get-RequiredCheckFailure -NeedsJson '' } |
            Should -Throw '*NEEDS_JSON is empty*'
    }
}

Describe 'Assert-RequiredCheck' {
    It 'does not throw when every job succeeded or skipped' {
        $json = '{"delta":{"result":"success"},"careful":{"result":"skipped"}}'
        { Assert-RequiredCheck -NeedsJson $json } | Should -Not -Throw
    }

    It 'throws naming the failing jobs' {
        $json = '{"validate-versions":{"result":"failure"}}'
        { Assert-RequiredCheck -NeedsJson $json } |
            Should -Throw '*validate-versions=failure*'
    }
}
