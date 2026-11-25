#Requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Pester suite for ArtifactVisibility.psm1. REST and job responses are mocked so selector,
# pagination, freshness, retry and permanent-failure boundaries run without GitHub or delays.
# Ref: .github/workflows/implementation.md#same-run-artifact-visibility.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'ArtifactVisibility.psm1') -Force

    function Get-TestArtifact(
        [long] $Id = 1,
        [string] $Name = 'report',
        [string] $CreatedAt = '2026-11-25T03:00:00Z',
        [bool] $Expired = $false,
        [long] $WorkflowRunId = 100
    ) {
        [pscustomobject]@{
            id = $Id
            name = $Name
            created_at = $CreatedAt
            expired = $Expired
            archive_download_url = "https://api.github.test/artifacts/$Id/zip"
            workflow_run = [pscustomobject]@{ id = $WorkflowRunId }
        }
    }

    function Get-TestHttpException([int] $StatusCode, [string] $Message = 'request failed') {
        $exception = [System.InvalidOperationException]::new($Message)
        $exception | Add-Member -NotePropertyName Response `
            -NotePropertyValue ([pscustomobject]@{ StatusCode = $StatusCode })
        $exception
    }
}

Describe 'Wait-GitHubRunArtifact' {
    BeforeEach {
        Mock Start-Sleep -ModuleName Retry { }
    }

    It 'waits for an exact artifact ID' {
        Mock Get-GitHubRunArtifact -ModuleName ArtifactVisibility {
            Get-TestArtifact -Id 42
        }

        {
            Wait-GitHubRunArtifact -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -GitHubToken token `
                -ArtifactId 42 -Attempt 2 -DelaySeconds 0
        } | Should -Not -Throw
        Should -Invoke Get-GitHubRunArtifact -ModuleName ArtifactVisibility -Times 1 -Exactly
    }

    It 'waits for the latest record with an exact artifact name' {
        Mock Get-GitHubRunArtifact -ModuleName ArtifactVisibility {
            Get-TestArtifact -Id 1 -Name report
            Get-TestArtifact -Id 2 -Name report
        }

        {
            Wait-GitHubRunArtifact -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -GitHubToken token `
                -Name report -Attempt 2 -DelaySeconds 0
        } | Should -Not -Throw
    }

    It 'waits through a temporarily disappearing partial pattern inventory' {
        $script:reads = 0
        Mock Get-GitHubRunArtifact -ModuleName ArtifactVisibility {
            $script:reads++
            if ($script:reads -eq 2) { return }
            Get-TestArtifact -Id 1 -Name book-alpha
            if ($script:reads -eq 3) {
                Get-TestArtifact -Id 2 -Name book-beta
            }
        }
        Mock Get-GitHubRunAttemptJob -ModuleName ArtifactVisibility { @() }
        $producers = @{
            'book-alpha' = 'Build book alpha'
            'book-beta' = 'Build book beta'
        }

        {
            Wait-GitHubRunArtifact -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -RunAttempt 2 -GitHubToken token `
                -Pattern 'book-*' -ExpectedCount 2 -ProducerJob $producers `
                -Attempt 4 -DelaySeconds 0
        } | Should -Not -Throw
        Should -Invoke Get-GitHubRunArtifact -ModuleName ArtifactVisibility -Times 3 -Exactly
    }

    It 'counts distinct pattern names rather than duplicate rerun records' {
        $script:reads = 0
        Mock Get-GitHubRunArtifact -ModuleName ArtifactVisibility {
            $script:reads++
            Get-TestArtifact -Id 1 -Name book-alpha
            Get-TestArtifact -Id 2 -Name book-alpha
            if ($script:reads -gt 1) {
                Get-TestArtifact -Id 3 -Name book-beta
            }
        }
        Mock Get-GitHubRunAttemptJob -ModuleName ArtifactVisibility { @() }
        $producers = @{
            'book-alpha' = 'Build book alpha'
            'book-beta' = 'Build book beta'
        }

        {
            Wait-GitHubRunArtifact -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -RunAttempt 2 -GitHubToken token `
                -Pattern 'book-*' -ExpectedCount 2 -ProducerJob $producers `
                -Attempt 3 -DelaySeconds 0
        } | Should -Not -Throw
        Should -Invoke Get-GitHubRunArtifact -ModuleName ArtifactVisibility -Times 2 -Exactly
    }

    It 'waits for artifacts newer than rerun producers' {
        $script:reads = 0
        Mock Get-GitHubRunArtifact -ModuleName ArtifactVisibility {
            $script:reads++
            $betaId = if ($script:reads -eq 1) { 2 } else { 3 }
            $betaCreatedAt = if ($script:reads -eq 1) {
                '2026-11-25T03:00:00Z'
            }
            else {
                '2026-11-25T04:30:00Z'
            }
            Get-TestArtifact -Id 1 -Name book-alpha -CreatedAt '2026-11-25T03:00:00Z'
            Get-TestArtifact -Id $betaId -Name book-beta -CreatedAt $betaCreatedAt
        }
        Mock Get-GitHubRunAttemptJob -ModuleName ArtifactVisibility {
            [pscustomobject]@{
                run_attempt = 2
                name = 'Build book beta'
                started_at = '2026-11-25T04:00:00Z'
            }
        }
        $producers = @{
            'book-alpha' = 'Build book alpha'
            'book-beta' = 'Build book beta'
        }

        {
            Wait-GitHubRunArtifact -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -RunAttempt 2 -GitHubToken token `
                -Pattern 'book-*' -ExpectedCount 2 -ProducerJob $producers `
                -Attempt 3 -DelaySeconds 0
        } | Should -Not -Throw
        Should -Invoke Get-GitHubRunArtifact -ModuleName ArtifactVisibility -Times 2 -Exactly
    }

    It 'accepts inherited artifacts from a reused producer job' {
        Mock Get-GitHubRunArtifact -ModuleName ArtifactVisibility {
            Get-TestArtifact -Id 1 -Name book-alpha
            Get-TestArtifact -Id 2 -Name book-beta
        }
        Mock Get-GitHubRunAttemptJob -ModuleName ArtifactVisibility {
            [pscustomobject]@{
                run_attempt = 2
                name = 'caller / Build book beta'
                started_at = '2026-11-25T02:30:00Z'
            }
        }
        $producers = @{
            'book-alpha' = 'Build book alpha'
            'book-beta' = 'Build book beta'
        }

        {
            Wait-GitHubRunArtifact -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -RunAttempt 2 -GitHubToken token `
                -Pattern 'book-*' -ExpectedCount 2 -ProducerJob $producers `
                -Attempt 2 -DelaySeconds 0
        } | Should -Not -Throw
    }

    It 'selects only producer-mapped names from a wider pattern inventory' {
        Mock Get-GitHubRunArtifact -ModuleName ArtifactVisibility {
            Get-TestArtifact -Id 1 -Name book-alpha
            Get-TestArtifact -Id 2 -Name book-beta
            Get-TestArtifact -Id 3 -Name book-removed
        }
        Mock Get-GitHubRunAttemptJob -ModuleName ArtifactVisibility { @() }
        $producers = @{
            'book-alpha' = 'Build book alpha'
            'book-beta' = 'Build book beta'
        }

        {
            Wait-GitHubRunArtifact -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -RunAttempt 2 -GitHubToken token `
                -Pattern 'book-*' -ExpectedCount 2 -ProducerJob $producers `
                -Attempt 2 -DelaySeconds 0
        } | Should -Not -Throw
    }

    It 'rejects a pattern producer map that does not match the expected count' {
        Mock Get-GitHubRunArtifact -ModuleName ArtifactVisibility {
            throw 'Inventory lookup must not run for an invalid selector contract.'
        }
        $producers = @{'book-alpha' = 'Build book alpha' }

        {
            Wait-GitHubRunArtifact -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -RunAttempt 2 -GitHubToken token `
                -Pattern 'book-*' -ExpectedCount 2 -ProducerJob $producers `
                -Attempt 2 -DelaySeconds 0
        } | Should -Throw -ExpectedMessage '*must equal*'
        Should -Invoke Get-GitHubRunArtifact -ModuleName ArtifactVisibility -Times 0 -Exactly
    }

    It 'fails after a pattern remains stale beyond the retry budget' {
        Mock Get-GitHubRunArtifact -ModuleName ArtifactVisibility {
            Get-TestArtifact -Id 1 -Name book-alpha -CreatedAt '2026-11-25T03:00:00Z'
        }
        Mock Get-GitHubRunAttemptJob -ModuleName ArtifactVisibility {
            [pscustomobject]@{
                run_attempt = 2
                name = 'Build book alpha'
                started_at = '2026-11-25T04:00:00Z'
            }
        }
        $producers = @{'book-alpha' = 'Build book alpha' }

        {
            Wait-GitHubRunArtifact -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -RunAttempt 2 -GitHubToken token `
                -Pattern 'book-*' -ExpectedCount 1 -ProducerJob $producers `
                -Attempt 3 -DelaySeconds 0
        } | Should -Throw -ExpectedMessage '*did not become visible after 3 attempts*'
        Should -Invoke Get-GitHubRunArtifact -ModuleName ArtifactVisibility -Times 3 -Exactly
    }

    It 'does not retry expired metadata' {
        Mock Get-GitHubRunArtifact -ModuleName ArtifactVisibility {
            Get-TestArtifact -Id 42 -Expired $true
        }

        {
            Wait-GitHubRunArtifact -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -GitHubToken token `
                -ArtifactId 42 -Attempt 4 -DelaySeconds 0
        } | Should -Throw -ExpectedMessage '*has expired*'
        Should -Invoke Get-GitHubRunArtifact -ModuleName ArtifactVisibility -Times 1 -Exactly
    }
}

Describe 'GitHub artifact HTTP boundaries' {
    BeforeEach {
        Mock Start-Sleep -ModuleName Retry { }
    }

    It 'retries an inventory from a real empty REST response' {
        $script:queries = 0
        Mock Invoke-RestMethod -ModuleName ArtifactVisibility {
            $script:queries++
            if ($script:queries -eq 1) {
                [pscustomobject]@{ total_count = 0; artifacts = @() }
            }
            else {
                [pscustomobject]@{
                    total_count = 1
                    artifacts = @(Get-TestArtifact -Id 42)
                }
            }
        }

        {
            Wait-GitHubRunArtifact -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -GitHubToken token `
                -ArtifactId 42 -RequestTimeoutSeconds 23 -Attempt 2 -DelaySeconds 0
        } | Should -Not -Throw
        Should -Invoke Invoke-RestMethod -ModuleName ArtifactVisibility -Times 2 -Exactly `
            -ParameterFilter { $TimeoutSec -eq 23 }
    }

    It 'uses the shared bounded retry defaults' {
        Mock Invoke-RestMethod -ModuleName ArtifactVisibility {
            [pscustomobject]@{ total_count = 0; artifacts = @() }
        }

        {
            Wait-GitHubRunArtifact -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -GitHubToken token -ArtifactId 42
        } | Should -Throw -ExpectedMessage '*after 12 attempts*'
        Should -Invoke Invoke-RestMethod -ModuleName ArtifactVisibility -Times 12 -Exactly `
            -ParameterFilter { $TimeoutSec -eq 30 }
        Should -Invoke Start-Sleep -ModuleName Retry -Times 11 -Exactly `
            -ParameterFilter { $Seconds -eq 5 }
    }

    It 'retries a transient HTTP 503 with the response diagnostics' {
        $script:queries = 0
        Mock Invoke-RestMethod -ModuleName ArtifactVisibility {
            $script:queries++
            if ($script:queries -eq 1) {
                throw (Get-TestHttpException -StatusCode 503 -Message 'Service Unavailable')
            }
            [pscustomobject]@{
                total_count = 1
                artifacts = @(Get-TestArtifact -Id 42)
            }
        }

        {
            Wait-GitHubRunArtifact -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -GitHubToken token `
                -ArtifactId 42 -Attempt 2 -DelaySeconds 0
        } | Should -Not -Throw
        Should -Invoke Invoke-RestMethod -ModuleName ArtifactVisibility -Times 2 -Exactly
    }

    It 'retries a statusless transport timeout' {
        $script:queries = 0
        Mock Invoke-RestMethod -ModuleName ArtifactVisibility {
            $script:queries++
            if ($script:queries -eq 1) {
                throw 'The operation timed out.'
            }
            [pscustomobject]@{
                total_count = 1
                artifacts = @(Get-TestArtifact -Id 42)
            }
        }

        {
            Wait-GitHubRunArtifact -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -GitHubToken token `
                -ArtifactId 42 -Attempt 2 -DelaySeconds 0
        } | Should -Not -Throw
        Should -Invoke Invoke-RestMethod -ModuleName ArtifactVisibility -Times 2 -Exactly
    }

    It 'does not retry an HTTP 401' {
        Mock Invoke-RestMethod -ModuleName ArtifactVisibility {
            throw (Get-TestHttpException -StatusCode 401 -Message 'Unauthorized')
        }

        {
            Wait-GitHubRunArtifact -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -GitHubToken token `
                -ArtifactId 42 -Attempt 4 -DelaySeconds 0
        } | Should -Throw
        Should -Invoke Invoke-RestMethod -ModuleName ArtifactVisibility -Times 1 -Exactly
    }
}
