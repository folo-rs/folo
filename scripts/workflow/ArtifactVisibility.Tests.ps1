#Requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Pester suite for ArtifactVisibility.psm1. Service responses and downloads are mocked so metadata,
# archive, rerun and HTTP-classification boundaries are exercised without GitHub or real delays.
# Ref: .github/workflows/implementation.md#same-run-artifact-visibility.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'ArtifactVisibility.psm1') -Force

    function Get-TestArtifact(
        [long] $Id = 1,
        [string] $Name = 'report',
        [string] $CreatedAt = '2026-11-25T03:00:00Z'
    ) {
        [pscustomobject]@{
            id                  = $Id
            name                = $Name
            created_at          = $CreatedAt
            expired             = $false
            archive_download_url = "https://api.github.test/artifacts/$Id/zip"
            workflow_run        = [pscustomobject]@{ id = 100 }
        }
    }

    function Get-TestHttpException([int] $StatusCode, [string] $Message = 'request failed') {
        $exception = [System.InvalidOperationException]::new($Message)
        $exception | Add-Member -NotePropertyName Response `
            -NotePropertyValue ([pscustomobject]@{ StatusCode = $StatusCode })
        $exception
    }
}

Describe 'Invoke-GitHubRunArtifactDownload' {
    BeforeEach {
        $env:RUNNER_TEMP = $TestDrive
        Mock Start-Sleep -ModuleName Retry { }
        Mock Save-GitHubArtifactArchive -ModuleName ArtifactVisibility {
            Set-Content -LiteralPath $Path -Value archive
        }
        Mock Expand-Archive -ModuleName ArtifactVisibility { }
    }

    AfterEach {
        Remove-Item Env:\RUNNER_TEMP -ErrorAction SilentlyContinue
    }

    It 'downloads an exact artifact ID' {
        Mock Get-GitHubRunArtifact -ModuleName ArtifactVisibility {
            Get-TestArtifact -Id 42
        }
        $path = Join-Path $TestDrive 'id'

        {
            Invoke-GitHubRunArtifactDownload -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -GitHubToken token -Path $path `
                -ArtifactId 42 -Attempt 2 -DelaySeconds 0
        } | Should -Not -Throw
        Should -Invoke Save-GitHubArtifactArchive -ModuleName ArtifactVisibility -Times 1 -Exactly
        Should -Invoke Expand-Archive -ModuleName ArtifactVisibility -Times 1 -Exactly `
            -ParameterFilter { $DestinationPath -ceq $path }
    }

    It 'downloads the latest record for an exact artifact name' {
        Mock Get-GitHubRunArtifact -ModuleName ArtifactVisibility {
            Get-TestArtifact -Id 1 -Name report
            Get-TestArtifact -Id 2 -Name report
        }

        {
            Invoke-GitHubRunArtifactDownload -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -GitHubToken token `
                -Path (Join-Path $TestDrive 'name') -Name report -Attempt 2 -DelaySeconds 0
        } | Should -Not -Throw
        Should -Invoke Save-GitHubArtifactArchive -ModuleName ArtifactVisibility `
            -Times 1 -Exactly -ParameterFilter { $Artifact.id -eq 2 }
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
            Invoke-GitHubRunArtifactDownload -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -GitHubToken token `
                -Path (Join-Path $TestDrive 'pattern') -RunAttempt 2 -Pattern 'book-*' `
                -ExpectedCount 2 -ProducerJob $producers -MergeMultiple -Attempt 4 -DelaySeconds 0
        } | Should -Not -Throw
        Should -Invoke Get-GitHubRunArtifact -ModuleName ArtifactVisibility -Times 3 -Exactly
        Should -Invoke Save-GitHubArtifactArchive -ModuleName ArtifactVisibility -Times 2 -Exactly
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
            Invoke-GitHubRunArtifactDownload -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -GitHubToken token `
                -Path (Join-Path $TestDrive 'duplicates') -RunAttempt 2 -Pattern 'book-*' `
                -ExpectedCount 2 -ProducerJob $producers -MergeMultiple `
                -Attempt 3 -DelaySeconds 0
        } | Should -Not -Throw
        Should -Invoke Save-GitHubArtifactArchive -ModuleName ArtifactVisibility `
            -Times 2 -Exactly
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
            Invoke-GitHubRunArtifactDownload -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -RunAttempt 2 -GitHubToken token `
                -Path (Join-Path $TestDrive 'rerun') -Pattern 'book-*' -ExpectedCount 2 `
                -ProducerJob $producers -MergeMultiple -Attempt 3 -DelaySeconds 0
        } | Should -Not -Throw
        Should -Invoke Get-GitHubRunArtifact -ModuleName ArtifactVisibility -Times 2 -Exactly
        Should -Invoke Save-GitHubArtifactArchive -ModuleName ArtifactVisibility -Times 2 -Exactly
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
            Invoke-GitHubRunArtifactDownload -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -RunAttempt 2 -GitHubToken token `
                -Path (Join-Path $TestDrive 'reused') -Pattern 'book-*' -ExpectedCount 2 `
                -ProducerJob $producers -MergeMultiple -Attempt 2 -DelaySeconds 0
        } | Should -Not -Throw
        Should -Invoke Save-GitHubArtifactArchive -ModuleName ArtifactVisibility -Times 2 -Exactly
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
            Invoke-GitHubRunArtifactDownload -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -RunAttempt 2 -GitHubToken token `
                -Path (Join-Path $TestDrive 'mapped') -Pattern 'book-*' -ExpectedCount 2 `
                -ProducerJob $producers -MergeMultiple -Attempt 2 -DelaySeconds 0
        } | Should -Not -Throw
        Should -Invoke Save-GitHubArtifactArchive -ModuleName ArtifactVisibility `
            -Times 2 -Exactly -ParameterFilter { $Artifact.name -cne 'book-removed' }
    }

    It 'rejects a pattern producer map that does not match the expected count' {
        Mock Get-GitHubRunArtifact -ModuleName ArtifactVisibility {
            throw 'Inventory lookup must not run for an invalid selector contract.'
        }
        $producers = @{'book-alpha' = 'Build book alpha' }

        {
            Invoke-GitHubRunArtifactDownload -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -RunAttempt 2 -GitHubToken token `
                -Path (Join-Path $TestDrive 'invalid-producers') -Pattern 'book-*' `
                -ExpectedCount 2 -ProducerJob $producers -Attempt 2 -DelaySeconds 0
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
            Invoke-GitHubRunArtifactDownload -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -RunAttempt 2 -GitHubToken token `
                -Path (Join-Path $TestDrive 'stale') -Pattern 'book-*' `
                -ExpectedCount 1 -ProducerJob $producers -Attempt 3 -DelaySeconds 0
        } | Should -Throw -ExpectedMessage '*did not become downloadable after 3 attempts*'
        Should -Invoke Get-GitHubRunArtifact -ModuleName ArtifactVisibility -Times 3 -Exactly
        Should -Invoke Save-GitHubArtifactArchive -ModuleName ArtifactVisibility -Times 0 -Exactly
    }

    It 'retries an archive 404 without extracting a partial set' {
        $script:downloads = 0
        Mock Get-GitHubRunArtifact -ModuleName ArtifactVisibility {
            Get-TestArtifact -Id 1 -Name book-alpha
            Get-TestArtifact -Id 2 -Name book-beta
        }
        Mock Save-GitHubArtifactArchive -ModuleName ArtifactVisibility {
            $script:downloads++
            if ($script:downloads -eq 2) {
                $pending = [System.Management.Automation.ErrorRecord]::new(
                    [System.InvalidOperationException]::new('not found'),
                    'ArtifactVisibilityPending',
                    [System.Management.Automation.ErrorCategory]::ResourceUnavailable,
                    $null
                )
                throw $pending
            }
            Set-Content -LiteralPath $Path -Value archive
        }
        Mock Get-GitHubRunAttemptJob -ModuleName ArtifactVisibility { @() }
        $producers = @{
            'book-alpha' = 'Build book alpha'
            'book-beta' = 'Build book beta'
        }

        {
            Invoke-GitHubRunArtifactDownload -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -GitHubToken token `
                -Path (Join-Path $TestDrive 'archive-404') -RunAttempt 2 -Pattern 'book-*' `
                -ExpectedCount 2 -ProducerJob $producers -MergeMultiple `
                -Attempt 3 -DelaySeconds 0
        } | Should -Not -Throw
        Should -Invoke Save-GitHubArtifactArchive -ModuleName ArtifactVisibility -Times 4 -Exactly
        Should -Invoke Expand-Archive -ModuleName ArtifactVisibility -Times 2 -Exactly
    }
}

Describe 'GitHub artifact HTTP boundaries' {
    BeforeEach {
        $env:RUNNER_TEMP = $TestDrive
        Mock Start-Sleep -ModuleName Retry { }
        Mock Expand-Archive -ModuleName ArtifactVisibility { }
        Mock Invoke-WebRequest -ModuleName ArtifactVisibility {
            Set-Content -LiteralPath $OutFile -Value archive
        }
    }

    AfterEach {
        Remove-Item Env:\RUNNER_TEMP -ErrorAction SilentlyContinue
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
            Invoke-GitHubRunArtifactDownload -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -GitHubToken token `
                -Path (Join-Path $TestDrive 'empty-rest') -ArtifactId 42 `
                -RequestTimeoutSeconds 23 -Attempt 2 -DelaySeconds 0
        } | Should -Not -Throw
        Should -Invoke Invoke-RestMethod -ModuleName ArtifactVisibility -Times 2 -Exactly `
            -ParameterFilter { $TimeoutSec -eq 23 }
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
            Invoke-GitHubRunArtifactDownload -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -GitHubToken token `
                -Path (Join-Path $TestDrive 'http-503') -ArtifactId 42 `
                -Attempt 2 -DelaySeconds 0
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
            Invoke-GitHubRunArtifactDownload -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -GitHubToken token `
                -Path (Join-Path $TestDrive 'transport-timeout') -ArtifactId 42 `
                -Attempt 2 -DelaySeconds 0
        } | Should -Not -Throw
        Should -Invoke Invoke-RestMethod -ModuleName ArtifactVisibility -Times 2 -Exactly
    }

    It 'does not retry an HTTP 401' {
        Mock Invoke-RestMethod -ModuleName ArtifactVisibility {
            throw (Get-TestHttpException -StatusCode 401 -Message 'Unauthorized')
        }

        {
            Invoke-GitHubRunArtifactDownload -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -GitHubToken token `
                -Path (Join-Path $TestDrive 'http-401') -ArtifactId 42 `
                -Attempt 4 -DelaySeconds 0
        } | Should -Throw
        Should -Invoke Invoke-RestMethod -ModuleName ArtifactVisibility -Times 1 -Exactly
    }

    It 'retries a real archive 404' {
        Mock Get-GitHubRunArtifact -ModuleName ArtifactVisibility {
            Get-TestArtifact -Id 42
        }
        $script:archives = 0
        Mock Invoke-WebRequest -ModuleName ArtifactVisibility {
            $script:archives++
            if ($script:archives -eq 1) {
                throw (Get-TestHttpException -StatusCode 404 -Message 'Not Found')
            }
            Set-Content -LiteralPath $OutFile -Value archive
        }

        {
            Invoke-GitHubRunArtifactDownload -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -GitHubToken token `
                -Path (Join-Path $TestDrive 'archive-404') -ArtifactId 42 `
                -Attempt 2 -DelaySeconds 0 -RequestTimeoutSeconds 21
        } | Should -Not -Throw
        Should -Invoke Invoke-WebRequest -ModuleName ArtifactVisibility -Times 2 -Exactly `
            -ParameterFilter { $TimeoutSec -eq 21 }
    }

    It 'does not retry an archive authorization failure' {
        Mock Get-GitHubRunArtifact -ModuleName ArtifactVisibility {
            Get-TestArtifact -Id 42
        }
        Mock Invoke-WebRequest -ModuleName ArtifactVisibility {
            throw (Get-TestHttpException -StatusCode 403 -Message 'Forbidden')
        }

        {
            Invoke-GitHubRunArtifactDownload -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -GitHubToken token `
                -Path (Join-Path $TestDrive 'archive-403') -ArtifactId 42 `
                -Attempt 4 -DelaySeconds 0
        } | Should -Throw
        Should -Invoke Invoke-WebRequest -ModuleName ArtifactVisibility -Times 1 -Exactly
    }
}
