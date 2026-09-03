#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Pester suite for RequiredChecks.psm1. The allowed-result policy (must-succeed vs may-skip)
# is the contract the Validation `required-checks` fan-in publishes to GitHub, so it is
# exercised against realistic `toJSON(needs)` payloads here rather than only in CI.

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'RequiredChecks.psm1') -Force
}

Describe 'Get-RequiredCheckFailure' {
    It 'returns nothing when every job succeeded' {
        InModuleScope RequiredChecks {
            $json = '{"delta":{"result":"success"},"test-scripts":{"result":"success"}}'
            Get-RequiredCheckFailure -NeedsJson $json -MustSucceedJob @('delta', 'test-scripts') |
                Should -BeNullOrEmpty
        }
    }

    It 'treats skipped as allowed for a job that may skip' {
        InModuleScope RequiredChecks {
            $json = '{"delta":{"result":"success"},"test-arm":{"result":"skipped"}}'
            Get-RequiredCheckFailure -NeedsJson $json -MustSucceedJob @('delta') |
                Should -BeNullOrEmpty
        }
    }

    It 'rejects skipped for an unconditional gate' {
        InModuleScope RequiredChecks {
            $json = '{"validate-versions":{"result":"skipped"},"test-arm":{"result":"skipped"}}'
            Get-RequiredCheckFailure -NeedsJson $json -MustSucceedJob @('validate-versions') |
                Should -Be @('validate-versions=skipped')
        }
    }

    It 'reports a failed job as job=result' {
        InModuleScope RequiredChecks {
            $json = '{"delta":{"result":"success"},"clippy-dev":{"result":"failure"}}'
            Get-RequiredCheckFailure -NeedsJson $json -MustSucceedJob @('delta') |
                Should -Be @('clippy-dev=failure')
        }
    }

    It 'reports cancelled and other non-allowed results' {
        InModuleScope RequiredChecks {
            $json = '{"mutants":{"result":"cancelled"},"hack":{"result":"neutral"}}'
            $result = @(Get-RequiredCheckFailure -NeedsJson $json -MustSucceedJob @('delta'))
            $result | Should -Contain 'mutants=cancelled'
            $result | Should -Contain 'hack=neutral'
        }
    }

    It 'treats a missing result property as a failure' {
        InModuleScope RequiredChecks {
            $json = '{"delta":{"outputs":{}}}'
            Get-RequiredCheckFailure -NeedsJson $json -MustSucceedJob @('delta') |
                Should -Be @('delta=missing')
        }
    }

    It 'throws when the payload is empty' {
        InModuleScope RequiredChecks {
            { Get-RequiredCheckFailure -NeedsJson '' -MustSucceedJob @('delta') } |
                Should -Throw '*NEEDS_JSON is empty*'
        }
    }

    It 'throws when the payload is an empty object' {
        InModuleScope RequiredChecks {
            { Get-RequiredCheckFailure -NeedsJson '{}' -MustSucceedJob @('delta') } |
                Should -Throw '*has no jobs*'
        }
    }

    It 'reports a must-succeed job that is absent from the needs payload' {
        InModuleScope RequiredChecks {
            $json = '{"test-scripts":{"result":"success"}}'
            Get-RequiredCheckFailure -NeedsJson $json -MustSucceedJob @('delta', 'test-scripts') |
                Should -Be @('delta=absent')
        }
    }

    It 'reports every absent must-succeed job in a deterministic order' {
        InModuleScope RequiredChecks {
            $json = '{"delta":{"result":"success"}}'
            $job = @('semver-checks', 'delta', 'validate-versions')
            Get-RequiredCheckFailure -NeedsJson $json -MustSucceedJob $job |
                Should -Be @('semver-checks=absent', 'validate-versions=absent')
        }
    }
}

Describe 'Assert-RequiredCheck' {
    It 'does not throw when every job succeeded or skipped where allowed' {
        $json = '{"delta":{"result":"success"},"careful":{"result":"skipped"}}'
        { Assert-RequiredCheck -NeedsJson $json -MustSucceedJob @('delta') } | Should -Not -Throw
    }

    It 'throws naming the failing jobs without a count prefix' {
        $json = '{"validate-versions":{"result":"failure"}}'
        { Assert-RequiredCheck -NeedsJson $json -MustSucceedJob @('validate-versions') } |
            Should -Throw '*did not produce an allowed result: validate-versions=failure*'
    }

    It 'throws when a must-succeed job drifted out of the needs list' {
        $json = '{"delta":{"result":"success"}}'
        { Assert-RequiredCheck -NeedsJson $json -MustSucceedJob @('delta', 'validate-versions') } |
            Should -Throw '*validate-versions=absent*'
    }

    It 'throws when the must-succeed list is empty' {
        $json = '{"delta":{"result":"success"}}'
        { Assert-RequiredCheck -NeedsJson $json -MustSucceedJob @() } |
            Should -Throw '*MUST_SUCCEED_JOBS is empty*'
    }
}
