#Requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Pester suite for ArtifactVisibility.psm1. REST and job responses are mocked so selector,
# pagination, freshness, retry and permanent-failure boundaries run without GitHub or delays.
# Ref: .github/workflows/implementation.md#same-run-artifact-visibility.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot '..\workflow\ArtifactVisibility.psm1') -Force

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

    It 'returns the frozen selection only after every exact ID is visible' {
        $script:reads = 0
        Mock Get-GitHubRunArtifact -ModuleName ArtifactVisibility {
            $script:reads++
            Get-TestArtifact -Id 41 -Name first
            if ($script:reads -gt 1) { Get-TestArtifact -Id 42 -Name second }
        }

        $selection = @(Wait-GitHubRunArtifact -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -GitHubToken token `
                -ArtifactIds '41,42' -Attempt 2 -DelaySeconds 0)

        $selection.Id | Should -Be @(41, 42)
        $selection.Name | Should -Be @('first', 'second')
        Should -Invoke Get-GitHubRunArtifact -ModuleName ArtifactVisibility -Times 2 -Exactly
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

    It 'retries transient metadata status <StatusCode>' -ForEach @(
        @{ StatusCode = 404 }
        @{ StatusCode = 408 }
        @{ StatusCode = 429 }
        @{ StatusCode = 500 }
        @{ StatusCode = 502 }
        @{ StatusCode = 503 }
        @{ StatusCode = 504 }
    ) {
        $script:queries = 0
        Mock Invoke-RestMethod -ModuleName ArtifactVisibility {
            $script:queries++
            if ($script:queries -eq 1) {
                throw (Get-TestHttpException -StatusCode $StatusCode -Message 'transient')
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

    It 'propagates a statusless transport failure' {
        $script:queries = 0
        Mock Invoke-RestMethod -ModuleName ArtifactVisibility {
            $script:queries++
            throw 'The operation timed out.'
        }

        {
            Wait-GitHubRunArtifact -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -GitHubToken token `
                -ArtifactId 42 -Attempt 4 -DelaySeconds 0
        } | Should -Throw '*timed out*'
        Should -Invoke Invoke-RestMethod -ModuleName ArtifactVisibility -Times 1 -Exactly
        Should -Invoke Start-Sleep -ModuleName Retry -Times 0 -Exactly
    }

    It 'does not retry permanent metadata status <StatusCode>' -ForEach @(
        @{ StatusCode = 401 }
        @{ StatusCode = 403 }
        @{ StatusCode = 410 }
    ) {
        Mock Invoke-RestMethod -ModuleName ArtifactVisibility {
            throw (Get-TestHttpException -StatusCode $StatusCode -Message 'permanent')
        }

        {
            Wait-GitHubRunArtifact -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -RunId 100 -GitHubToken token `
                -ArtifactId 42 -Attempt 4 -DelaySeconds 0
        } | Should -Throw
        Should -Invoke Invoke-RestMethod -ModuleName ArtifactVisibility -Times 1 -Exactly
    }
}

Describe 'Artifact transfer retry and completeness' {
    BeforeEach {
        Mock Start-Sleep -ModuleName ArtifactVisibility { }
    }

    It 'checks every archive with the workflow token and retries only a delayed redirect' {
        $script:checks = 0
        Mock Get-GitHubArtifactArchiveStatus -ModuleName ArtifactVisibility {
            $script:checks++
            if ($Uri -ceq 'https://api.github.test/repos/owner/repo/actions/artifacts/42/zip' -and
                $script:checks -eq 2) {
                return 404
            }
            302
        }

        Wait-GitHubArtifactArchiveRedirect -ApiUrl 'https://api.github.test' `
            -Repository owner/repo -GitHubToken job-token -ArtifactIds 41, 42 `
            -Attempt 2 -DelaySeconds 0 | Should -BeTrue

        Should -Invoke Get-GitHubArtifactArchiveStatus -ModuleName ArtifactVisibility `
            -Times 3 -Exactly
        Should -Invoke Get-GitHubArtifactArchiveStatus -ModuleName ArtifactVisibility `
            -Times 3 -Exactly -ParameterFilter {
                $Headers.Authorization -ceq 'Bearer job-token'
            }
        Should -Invoke Start-Sleep -ModuleName ArtifactVisibility -Times 1 -Exactly
    }

    It 'falls back to same-run transfer after persistent REST 404 for one immutable ID' {
        Mock Get-GitHubArtifactArchiveStatus -ModuleName ArtifactVisibility { 404 }

        Wait-GitHubArtifactArchiveRedirect -ApiUrl 'https://api.github.test' `
            -Repository owner/repo -GitHubToken job-token -ArtifactIds 41 `
            -Attempt 12 -DelaySeconds 0 | Should -BeFalse
        Should -Invoke Get-GitHubArtifactArchiveStatus -ModuleName ArtifactVisibility `
            -Times 12 -Exactly
    }

    It 'rejects an incomplete multi-ID transfer instead of using a masked 404 fallback' {
        Mock Get-GitHubArtifactArchiveStatus -ModuleName ArtifactVisibility { 404 }

        {
            Wait-GitHubArtifactArchiveRedirect -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -GitHubToken job-token -ArtifactIds 41, 42 `
                -Attempt 2 -DelaySeconds 0
        } | Should -Throw '*artifact 41*HTTP 404*attempt 2/2*'
    }

    It 'propagates permanent archive status <Status> without waiting' -ForEach @(
        @{ Status = 401 }
        @{ Status = 403 }
        @{ Status = 410 }
    ) {
        Mock Get-GitHubArtifactArchiveStatus -ModuleName ArtifactVisibility { $Status }

        {
            Wait-GitHubArtifactArchiveRedirect -ApiUrl 'https://api.github.test' `
                -Repository owner/repo -GitHubToken job-token -ArtifactIds 41 `
                -Attempt 4 -DelaySeconds 0
        } | Should -Throw "*HTTP $Status*attempt 1/*"
        Should -Invoke Start-Sleep -ModuleName ArtifactVisibility -Times 0 -Exactly
    }

    It 'rejects missing or unexpected multi-artifact staging entries' {
        $selection = @(
            [pscustomobject]@{ Id = 41; Name = 'first' }
            [pscustomobject]@{ Id = 42; Name = 'second' }
        )
        $stage = New-Item -ItemType Directory -Path (Join-Path $TestDrive stage)
        $null = New-Item -ItemType Directory -Path (Join-Path $stage first)
        { Assert-GitHubArtifactTransferComplete -Path $stage -Selection $selection } |
            Should -Throw '*missing second*'
        $null = New-Item -ItemType Directory -Path (Join-Path $stage second)
        $null = New-Item -ItemType File -Path (Join-Path $stage unexpected)
        { Assert-GitHubArtifactTransferComplete -Path $stage -Selection $selection } |
            Should -Throw '*unexpected staged entry*'
    }

    It 'exposes the flattened contents of a complete multi-artifact staging tree' {
        $selection = @(
            [pscustomobject]@{ Id = 41; Name = 'book-alpha' }
            [pscustomobject]@{ Id = 42; Name = 'book-beta' }
        )
        $stage = New-Item -ItemType Directory -Path (Join-Path $TestDrive complete-stage)
        $alpha = New-Item -ItemType Directory -Path (Join-Path $stage 'book-alpha')
        $beta = New-Item -ItemType Directory -Path (Join-Path $stage 'book-beta')
        Set-Content -LiteralPath (Join-Path $alpha 'alpha.html') -Value alpha
        Set-Content -LiteralPath (Join-Path $beta 'beta.html') -Value beta
        $destination = Join-Path $TestDrive public

        Publish-GitHubArtifactTransfer -Source $stage -Destination $destination `
            -Selection $selection -MergeMultiple

        (Get-Content -LiteralPath (Join-Path $destination 'alpha.html')) | Should -Be alpha
        (Get-Content -LiteralPath (Join-Path $destination 'beta.html')) | Should -Be beta
        Test-Path -LiteralPath $stage | Should -BeFalse
    }
}
