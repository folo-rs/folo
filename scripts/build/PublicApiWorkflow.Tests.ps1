#Requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Protects the producer/consumer workflow contract without restoring unrelated legacy
# workflow checks. Structural assertions catch a broken artifact handoff or trust boundary.
# Ref: ../../.github/workflows/implementation.md#public-api-diff-publication.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    $root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
    $script:producer = Get-Content -LiteralPath (Join-Path $root '.github/workflows/public-api-diff.yml') -Raw
    $script:publisher = Get-Content -LiteralPath (Join-Path $root '.github/workflows/public-api-diff-publish.yml') -Raw
}

Describe 'Public API workflow contract' {
    It 'connects the read-only producer name to the trusted publisher' {
        $name = [regex]::Match($producer, '(?m)^name: (?<name>[^\r\n]+)').Groups['name'].Value
        $name | Should -BeExactly 'Public API diff'
        $publisher | Should -Match ([regex]::Escape("- $name"))
        $producer | Should -Match '(?m)^on:\r?\n  pull_request:\r?\n'
        foreach ($eventType in @('opened', 'synchronize', 'reopened', 'ready_for_review', 'edited')) {
            $producer | Should -Match ([regex]::Escape($eventType))
        }
        $producer | Should -Match '(?m)^  contents: read\r?$'
        $producer | Should -Not -Match '(?m)^\s+[a-z-]+: write\r?$'
    }

    It 'checks out the frozen head with complete history and no persisted credentials' {
        $producer | Should -Match 'ref: \$\{\{ github\.event\.pull_request\.head\.sha \}\}'
        $producer | Should -Match '(?m)^\s+fetch-depth: 0\r?$'
        $producer | Should -Match '(?m)^\s+persist-credentials: false\r?$'
        $producer | Should -Match 'BASE_SHA: \$\{\{ github\.event\.pull_request\.base\.sha \}\}'
        $producer | Should -Match 'HEAD_SHA: \$\{\{ github\.event\.pull_request\.head\.sha \}\}'
    }

    It 'uploads the exact versioned report on every producer outcome' {
        $artifact = 'public-api-diff-\$\{\{ github\.run_id \}\}-\$\{\{ github\.run_attempt \}\}'
        $producer | Should -Match "name: $artifact"
        $producer | Should -Match '(?m)^\s+if: always\(\)\r?$'
        $producer | Should -Match 'public-api-diff/public-api-diff\.json'
        $publisher | Should -Match 'name: public-api-diff-\$\{\{ github\.event\.workflow_run\.id \}\}-\$\{\{ github\.event\.workflow_run\.run_attempt \}\}'
        $publisher | Should -Match 'public-api-diff/public-api-diff\.json'
    }
}
