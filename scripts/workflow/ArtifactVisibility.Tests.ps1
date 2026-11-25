#Requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Pester suite for ArtifactVisibility.psm1. Artifact inventory and sleep operations are mocked so
# selector behavior, bounded retries and permanent-failure handling run without GitHub or delays.
# Ref: .github/workflows/implementation.md#same-run-artifact-visibility.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'ArtifactVisibility.psm1') -Force
}

Describe 'Wait-GitHubRunArtifact' {
    BeforeEach {
        Mock Start-Sleep -ModuleName Retry { }
    }

    It 'waits for an artifact ID through the paginated run inventory' {
        Mock gh -ModuleName ArtifactVisibility {
            $global:LASTEXITCODE = 0
            '[{"artifacts":[{"id":42,"name":"report","expired":false}]}]'
        }

        {
            Wait-GitHubRunArtifact -Repository owner/repo -RunId 100 -ArtifactId 42 `
                -Attempt 2 -DelaySeconds 0
        } | Should -Not -Throw
        Should -Invoke gh -ModuleName ArtifactVisibility -Times 1 -Exactly
    }

    It 'waits for an exact artifact name' {
        Mock Get-GitHubRunArtifact -ModuleName ArtifactVisibility {
            [pscustomobject]@{ id = 7; name = 'report' }
        }

        {
            Wait-GitHubRunArtifact -Repository owner/repo -RunId 100 -Name report `
                -Attempt 2 -DelaySeconds 0
        } | Should -Not -Throw
    }

    It 'waits until a pattern has every expected artifact' {
        $script:reads = 0
        Mock Get-GitHubRunArtifact -ModuleName ArtifactVisibility {
            $script:reads++
            [pscustomobject]@{ id = 1; name = 'book-alpha' }
            if ($script:reads -gt 1) {
                [pscustomobject]@{ id = 2; name = 'book-beta' }
            }
        }

        {
            Wait-GitHubRunArtifact -Repository owner/repo -RunId 100 -Pattern 'book-*' `
                -ExpectedCount 2 -Attempt 3 -DelaySeconds 0
        } | Should -Not -Throw
        Should -Invoke Get-GitHubRunArtifact -ModuleName ArtifactVisibility -Times 2 -Exactly
    }

    It 'does not count duplicate pattern names as complete inventory' {
        $script:reads = 0
        Mock Get-GitHubRunArtifact -ModuleName ArtifactVisibility {
            $script:reads++
            [pscustomobject]@{ id = 1; name = 'book-alpha' }
            [pscustomobject]@{ id = 2; name = 'book-alpha' }
            if ($script:reads -gt 1) {
                [pscustomobject]@{ id = 3; name = 'book-beta' }
            }
        }

        {
            Wait-GitHubRunArtifact -Repository owner/repo -RunId 100 -Pattern 'book-*' `
                -ExpectedCount 2 -Attempt 3 -DelaySeconds 0
        } | Should -Not -Throw
        Should -Invoke Get-GitHubRunArtifact -ModuleName ArtifactVisibility -Times 2 -Exactly
    }

    It 'waits for artifacts newer than producers rerun in the current attempt' {
        $script:reads = 0
        Mock Get-GitHubRunArtifact -ModuleName ArtifactVisibility {
            $script:reads++
            [pscustomobject]@{
                id         = 1
                name       = 'book-alpha'
                created_at = '2026-11-25T03:00:00Z'
            }
            [pscustomobject]@{
                id         = if ($script:reads -eq 1) { 2 } else { 3 }
                name       = 'book-beta'
                created_at = if ($script:reads -eq 1) {
                    '2026-11-25T03:00:00Z'
                } else {
                    '2026-11-25T04:30:00Z'
                }
            }
        }
        Mock Get-GitHubRunAttemptJob -ModuleName ArtifactVisibility {
            [pscustomobject]@{
                run_attempt = 2
                name        = 'Build book beta'
                started_at  = '2026-11-25T04:00:00Z'
            }
        }
        $producerJobs = @{
            'book-alpha' = 'Build book alpha'
            'book-beta'  = 'Build book beta'
        }

        {
            Wait-GitHubRunArtifact -Repository owner/repo -RunId 100 -RunAttempt 2 `
                -Pattern 'book-*' -ExpectedCount 2 -ProducerJob $producerJobs `
                -Attempt 3 -DelaySeconds 0
        } | Should -Not -Throw
        Should -Invoke Get-GitHubRunArtifact -ModuleName ArtifactVisibility -Times 2 -Exactly
        Should -Invoke Get-GitHubRunAttemptJob -ModuleName ArtifactVisibility -Times 2 -Exactly
    }

    It 'accepts inherited producers that did not run in the current attempt' {
        Mock Get-GitHubRunArtifact -ModuleName ArtifactVisibility {
            [pscustomobject]@{
                id         = 1
                name       = 'book-alpha'
                created_at = '2026-11-25T03:00:00Z'
            }
            [pscustomobject]@{
                id         = 3
                name       = 'book-beta'
                created_at = '2026-11-25T04:30:00Z'
            }
        }
        Mock Get-GitHubRunAttemptJob -ModuleName ArtifactVisibility {
            [pscustomobject]@{
                run_attempt = 2
                name        = 'Build book beta'
                started_at  = '2026-11-25T04:00:00Z'
            }
        }
        $producerJobs = @{
            'book-alpha' = 'Build book alpha'
            'book-beta'  = 'Build book beta'
        }

        {
            Wait-GitHubRunArtifact -Repository owner/repo -RunId 100 -RunAttempt 2 `
                -Pattern 'book-*' -ExpectedCount 2 -ProducerJob $producerJobs `
                -Attempt 2 -DelaySeconds 0
        } | Should -Not -Throw
        Should -Invoke Get-GitHubRunArtifact -ModuleName ArtifactVisibility -Times 1 -Exactly
    }

    It 'fails explicitly after the visibility budget is exhausted' {
        Mock Get-GitHubRunArtifact -ModuleName ArtifactVisibility { @() }

        {
            Wait-GitHubRunArtifact -Repository owner/repo -RunId 100 -ArtifactId 42 `
                -Attempt 3 -DelaySeconds 0
        } | Should -Throw -ExpectedMessage '*did not become visible after 3 attempts*'
        Should -Invoke Get-GitHubRunArtifact -ModuleName ArtifactVisibility -Times 3 -Exactly
    }

    It 'retries a transient inventory query failure' {
        $script:queries = 0
        Mock gh -ModuleName ArtifactVisibility {
            $script:queries++
            if ($script:queries -eq 1) {
                $global:LASTEXITCODE = 1
                'HTTP 503: Service Unavailable'
            } else {
                $global:LASTEXITCODE = 0
                '[{"artifacts":[{"id":42,"name":"report","expired":false}]}]'
            }
        }

        {
            Wait-GitHubRunArtifact -Repository owner/repo -RunId 100 -ArtifactId 42 `
                -Attempt 2 -DelaySeconds 0
        } | Should -Not -Throw
        Should -Invoke gh -ModuleName ArtifactVisibility -Times 2 -Exactly
    }

    It 'does not retry a permanent inventory query failure' {
        Mock Get-GitHubRunArtifact -ModuleName ArtifactVisibility {
            throw 'gh: authentication required'
        }

        {
            Wait-GitHubRunArtifact -Repository owner/repo -RunId 100 -ArtifactId 42 `
                -Attempt 4 -DelaySeconds 0
        } | Should -Throw -ExpectedMessage '*authentication required*'
        Should -Invoke Get-GitHubRunArtifact -ModuleName ArtifactVisibility -Times 1 -Exactly
    }
}
