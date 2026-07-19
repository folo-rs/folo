#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Pester suite for Delta.psm1. The two pure functions carry the parsing/shaping logic both the
# `just delta*` recipes and the CI `delta` job depend on, so they are exercised directly here:
# Read-DeltaAffectedPackage against realistic `cargo delta run` JSON (including the "nothing
# affected" and malformed-but-tolerated shapes that must not throw under strict mode), and
# Get-DeltaOutput against the three CI step outputs it produces. Invoke-CargoDelta drives real
# cargo/git, so it is covered by the recipes running in CI rather than unit-tested here.

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'Delta.psm1') -Force
}

Describe 'Read-DeltaAffectedPackage' {
    It 'returns the affected package names' {
        $json = '{"Affected":["events_once","infinity_pool"]}'
        $result = Read-DeltaAffectedPackage -DeltaJson $json
        $result | Should -Be @('events_once', 'infinity_pool')
    }

    It 'returns a single affected package as a one-element array' {
        $result = @(Read-DeltaAffectedPackage -DeltaJson '{"Affected":["events"]}')
        $result.Count | Should -Be 1
        $result[0] | Should -Be 'events'
    }

    It 'returns an empty array when the affected list is empty' {
        Read-DeltaAffectedPackage -DeltaJson '{"Affected":[]}' | Should -BeNullOrEmpty
    }

    It 'returns an empty array when there is no Affected field (strict-mode safe)' {
        Read-DeltaAffectedPackage -DeltaJson '{"Other":1}' | Should -BeNullOrEmpty
    }

    It 'returns an empty array for a null Affected field' {
        Read-DeltaAffectedPackage -DeltaJson '{"Affected":null}' | Should -BeNullOrEmpty
    }

    It 'ignores unrelated fields in the report' {
        $json = '{"Affected":["par_bench"],"Summary":"whatever","Count":1}'
        Read-DeltaAffectedPackage -DeltaJson $json | Should -Be @('par_bench')
    }
}

Describe 'Select-ExistingPackage' {
    It 'drops packages that no longer exist in the workspace' {
        $result = Select-ExistingPackage `
            -Affected @('mock_bench_engine', 'cargo-bench-history-faker', 'cbh_engines') `
            -WorkspacePackage @('cargo-bench-history-faker', 'cbh_engines', 'cbh_cli')
        $result | Should -Be @('cargo-bench-history-faker', 'cbh_engines')
    }

    It 'preserves the affected order' {
        $result = Select-ExistingPackage `
            -Affected @('c', 'a', 'b') `
            -WorkspacePackage @('a', 'b', 'c')
        $result | Should -Be @('c', 'a', 'b')
    }

    It 'returns an empty array when every affected package was removed' {
        Select-ExistingPackage -Affected @('gone') -WorkspacePackage @('here') |
            Should -BeNullOrEmpty
    }

    It 'returns an empty array for an empty affected list' {
        Select-ExistingPackage -Affected @() -WorkspacePackage @('a') | Should -BeNullOrEmpty
    }

    It 'is case-sensitive (a differently cased name is treated as absent)' {
        Select-ExistingPackage -Affected @('Events') -WorkspacePackage @('events') |
            Should -BeNullOrEmpty
    }
}

Describe 'Get-DeltaOutput' {
    Context 'when packages are affected' {
        BeforeAll {
            $script:Output = Get-DeltaOutput -Affected @('events_once', 'infinity_pool')
        }

        It 'joins packages with spaces' {
            $script:Output.Packages | Should -Be 'events_once infinity_pool'
        }

        It 'emits a JSON array of the package names' {
            $script:Output.PackagesJson | Should -Be '["events_once","infinity_pool"]'
        }

        It 'does not skip when something is affected' {
            $script:Output.SkipAll | Should -Be 'false'
        }
    }

    Context 'when nothing is affected' {
        BeforeAll {
            $script:Output = Get-DeltaOutput -Affected @()
        }

        It 'produces an empty package string' {
            $script:Output.Packages | Should -Be ''
        }

        It 'produces an empty JSON array' {
            $script:Output.PackagesJson | Should -Be '[]'
        }

        It 'signals skip_all' {
            $script:Output.SkipAll | Should -Be 'true'
        }
    }

    It 'produces valid JSON that round-trips to the original list' {
        $affected = @('a-b', 'c_d', 'e')
        $json = (Get-DeltaOutput -Affected $affected).PackagesJson
        $roundTripped = @($json | ConvertFrom-Json)
        ($roundTripped -join ',') | Should -Be ($affected -join ',')
    }
}
