#Requires -Version 7.6
#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Protects the trust boundary and idempotence of the workflow_run public-API comment
# publisher: schema mismatches, stale comparisons and duplicate markers must never be written.
# Ref: ../../.github/workflows/design.md#public-api-diff-publication.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot '..\workflow\PublicApiComment.psm1') -Force

    function Write-PublicApiArtifact {
        param(
            [hashtable] $Change = @{
                schema_version = 1
                repository = 'example/repository'
                pull_request = 42
                head_sha = ('a' * 40)
                base_sha = ('c' * 40)
                merge_base_sha = ('b' * 40)
                run_id = 123
                run_attempt = 1
                packages = @()
            }
        )

        $path = Join-Path $TestDrive 'public-api-diff.json'
        $Change | ConvertTo-Json -Depth 10 -Compress | Set-Content -LiteralPath $path -Encoding utf8
        return $path
    }
}

Describe 'Public API artifact validation' {
    It 'accepts a complete empty report' {
        $artifact = Read-PublicApiArtifact -Path (Write-PublicApiArtifact) `
            -Repository example/repository -PullRequest 42 -RunId 123 -RunAttempt 1 `
            -HeadSha ('a' * 40)

        $artifact.repository | Should -BeExactly 'example/repository'
        $artifact.base_sha | Should -Be ('c' * 40)
        @($artifact.packages).Count | Should -Be 0
    }

    It 'rejects event identity supplied by the artifact' {
        $change = @{
            schema_version = 1
            repository = 'example/other'
            pull_request = 42
            head_sha = ('a' * 40)
            base_sha = ('c' * 40)
            merge_base_sha = ('b' * 40)
            run_id = 123
            run_attempt = 1
            packages = @()
        }
        {
            Read-PublicApiArtifact -Path (Write-PublicApiArtifact -Change $change) `
                -Repository example/repository -PullRequest 42 -RunId 123 -RunAttempt 1 `
                -HeadSha ('a' * 40)
        } | Should -Throw '*repository*workflow repository*'
    }

    It 'rejects duplicate package reports' {
        $package = @{ name = 'sample'; status = 'unchanged'; output = '' }
        $change = @{
            schema_version = 1
            repository = 'example/repository'
            pull_request = 42
            head_sha = ('a' * 40)
            base_sha = ('c' * 40)
            merge_base_sha = ('b' * 40)
            run_id = 123
            run_attempt = 1
            packages = @($package, $package)
        }
        {
            Read-PublicApiArtifact -Path (Write-PublicApiArtifact -Change $change) `
                -Repository example/repository -PullRequest 42 -RunId 123 -RunAttempt 1 `
                -HeadSha ('a' * 40)
        } | Should -Throw '*repeats package*'
    }
}

Describe 'Public API comment rendering' {
    It 'renders an explicit no-change result' {
        $artifact = @{
            repository = 'example/repository'
            head_sha = ('a' * 40)
            packages = @()
        }
        $body = ConvertTo-PublicApiComment -Artifact $artifact -RunId 123 -RunAttempt 1 `
            -Conclusion success

        $body | Should -Match 'No public API changes\.'
        $body | Should -Match '<!-- folo-public-api-diff:v1 -->'
    }

    It 'uses a fence that cannot be terminated by tool output' {
        $artifact = @{
            repository = 'example/repository'
            head_sha = ('a' * 40)
            packages = @(
                @{
                    name = 'sample'
                    status = 'changed'
                    output = 'pub struct Example<T>;' + "`n" + '````'
                }
            )
        }
        $body = ConvertTo-PublicApiComment -Artifact $artifact -RunId 123 -RunAttempt 2 `
            -Conclusion success

        $body | Should -Match '(?m)^`````text$'
        $body | Should -Match '/actions/runs/123/attempts/2'
    }

    It 'bounds oversized fallback scope and retains incomplete generation status' {
        $artifact = @{
            repository = 'example/repository'
            head_sha = ('a' * 40)
            packages = @(
                @{ name = 'changed'; status = 'changed'; output = 'a' * 500 }
                @{ name = 'broken'; status = 'error'; output = 'failed' }
            )
        }
        $body = ConvertTo-PublicApiComment -Artifact $artifact -RunId 123 -RunAttempt 1 `
            -Conclusion success -MaximumBytes 100

        $body | Should -Match 'Changed packages: 1'
        $body | Should -Match 'Incomplete packages: 1'
        $body | Should -Not -Match '``changed``|``broken``'
        [Text.Encoding]::UTF8.GetByteCount($body) | Should -BeLessThan 1000
    }

    It 'qualifies a valid report from an unsuccessful producer' {
        $artifact = @{
            repository = 'example/repository'
            head_sha = ('a' * 40)
            packages = @()
        }
        $body = ConvertTo-PublicApiComment -Artifact $artifact -RunId 123 -RunAttempt 1 `
            -Conclusion failure

        $body | Should -Match 'No public API changes\.'
        $body | Should -Match 'concluded with `failure`'
        $body | Should -Match 'not authoritative'
    }
}

Describe 'Public API comment reconciliation' {
    BeforeEach {
        $script:marker = '<!-- folo-public-api-diff:v1 -->'
        $script:sha = 'a' * 40
        $script:baseSha = 'b' * 40
        $script:mergeBaseSha = 'd' * 40
        Mock Invoke-PublicApiGitHubJson -ModuleName PublicApiComment {
            param($Endpoint)
            if ($Endpoint -like '*/pulls/*') {
                return @{ head = @{ sha = $script:sha }; base = @{ sha = $script:baseSha } }
            }
            if ($Endpoint -like '*/compare/*') {
                return @{ merge_base_commit = @{ sha = $script:mergeBaseSha } }
            }
            return @{ id = 7; body = $script:publishedBody }
        }
    }

    It 'creates the first owned comment' {
        Mock Get-PublicApiGitHubCollection -ModuleName PublicApiComment { @() }
        $body = "$script:marker`n## Public API diff"
        $script:publishedBody = $body

        $comment = Publish-PublicApiPullRequestComment -Repository example/repository -PullRequest 42 `
            -HeadSha $script:sha -MergeBaseSha $script:mergeBaseSha `
            -RunId 123 -RunAttempt 1 -Body $body

        $comment.body | Should -BeExactly $body
        Should -Invoke Invoke-PublicApiGitHubJson -ModuleName PublicApiComment -Times 1 -Exactly `
            -ParameterFilter { $Method -eq 'POST' -and $Endpoint -eq 'repos/example/repository/issues/42/comments' }
    }

    It 'updates the single existing marker comment' {
        Mock Get-PublicApiGitHubCollection -ModuleName PublicApiComment {
            @(@{
                id = 9
                body = "$script:marker`nold"
                user = @{ login = 'github-actions[bot]' }
            })
        }
        $body = "$script:marker`n## Public API diff"
        $script:publishedBody = $body

        Publish-PublicApiPullRequestComment -Repository example/repository -PullRequest 42 `
            -HeadSha $script:sha -MergeBaseSha $script:mergeBaseSha `
            -RunId 123 -RunAttempt 1 -Body $body

        Should -Invoke Invoke-PublicApiGitHubJson -ModuleName PublicApiComment -Times 1 -Exactly `
            -ParameterFilter { $Method -eq 'PATCH' -and $Endpoint -eq 'repos/example/repository/issues/comments/9' }
    }

    It 'rejects duplicate marker ownership' {
        Mock Get-PublicApiGitHubCollection -ModuleName PublicApiComment {
            @(
                @{
                    id = 8
                    body = "$script:marker`nfirst"
                    user = @{ login = 'github-actions[bot]' }
                }
                @{
                    id = 9
                    body = "$script:marker`nsecond"
                    user = @{ login = 'github-actions[bot]' }
                }
            )
        }

        {
            Publish-PublicApiPullRequestComment -Repository example/repository -PullRequest 42 `
                -HeadSha $script:sha -MergeBaseSha $script:mergeBaseSha -RunId 123 -RunAttempt 1 `
                -Body "$script:marker`nreplacement"
        } | Should -Throw '*multiple public API diff marker comments*'
        Should -Invoke Invoke-PublicApiGitHubJson -ModuleName PublicApiComment -Times 0 -Exactly `
            -ParameterFilter { $Method -in @('POST', 'PATCH') }
    }

    It 'ignores a user-authored copy of the marker' {
        Mock Get-PublicApiGitHubCollection -ModuleName PublicApiComment {
            @(@{
                id = 9
                body = "$script:marker`nquoted by a reviewer"
                user = @{ login = 'contributor' }
            })
        }
        $body = "$script:marker`n## Public API diff"
        $script:publishedBody = $body

        Publish-PublicApiPullRequestComment -Repository example/repository -PullRequest 42 `
            -HeadSha $script:sha -MergeBaseSha $script:mergeBaseSha `
            -RunId 123 -RunAttempt 1 -Body $body

        Should -Invoke Invoke-PublicApiGitHubJson -ModuleName PublicApiComment -Times 1 -Exactly `
            -ParameterFilter { $Method -eq 'POST' -and $Endpoint -eq 'repos/example/repository/issues/42/comments' }
    }

    It 'refuses to write an older run over a newer head' {
        Mock Invoke-PublicApiGitHubJson -ModuleName PublicApiComment {
            param($Endpoint)
            $null = $Endpoint
            return @{ head = @{ sha = ('b' * 40) } }
        }
        Mock Get-PublicApiGitHubCollection -ModuleName PublicApiComment { @() }

        {
            Publish-PublicApiPullRequestComment -Repository example/repository -PullRequest 42 `
                -HeadSha $script:sha -MergeBaseSha $script:mergeBaseSha -RunId 123 -RunAttempt 1 `
                -Body "$script:marker`nold"
        } | Should -Throw '*advanced beyond workflow head*'
        Should -Invoke Invoke-PublicApiGitHubJson -ModuleName PublicApiComment -Times 0 -Exactly `
            -ParameterFilter { $Method -in @('POST', 'PATCH') }
    }

    It 'publishes a current comparison after an unrelated base-tip change' {
        $script:baseSha = 'c' * 40
        Mock Get-PublicApiGitHubCollection -ModuleName PublicApiComment { @() }

        Publish-PublicApiPullRequestComment -Repository example/repository -PullRequest 42 `
            -HeadSha $script:sha -MergeBaseSha $script:mergeBaseSha -RunId 123 -RunAttempt 1 `
            -Body "$script:marker`ncurrent"

        Should -Invoke Invoke-PublicApiGitHubJson -ModuleName PublicApiComment -Times 1 -Exactly `
            -ParameterFilter { $Method -eq 'POST' -and $Endpoint -eq 'repos/example/repository/issues/42/comments' }
    }

    It 'rejects a comparison after the effective merge base changes' {
        $script:mergeBaseSha = 'e' * 40
        Mock Get-PublicApiGitHubCollection -ModuleName PublicApiComment { @() }

        {
            Publish-PublicApiPullRequestComment -Repository example/repository -PullRequest 42 `
                -HeadSha $script:sha -MergeBaseSha ('d' * 40) -RunId 123 -RunAttempt 1 `
                -Body "$script:marker`nold"
        } | Should -Throw '*changed merge base since workflow comparison*'
        Should -Invoke Invoke-PublicApiGitHubJson -ModuleName PublicApiComment -Times 0 -Exactly `
            -ParameterFilter { $Method -in @('POST', 'PATCH') }
    }

    It 'preserves a newer report when an older run has no valid comparison' {
        $newerBody = ConvertTo-PublicApiComment -Artifact @{
            repository = 'example/repository'
            head_sha = $script:sha
            packages = @()
        } -RunId 200 -RunAttempt 1 -Conclusion success
        Mock Get-PublicApiGitHubCollection -ModuleName PublicApiComment {
            @(@{
                id = 9
                body = $newerBody
                user = @{ login = 'github-actions[bot]' }
            })
        }

        $comment = Publish-PublicApiPullRequestComment -Repository example/repository -PullRequest 42 `
            -HeadSha $script:sha -MergeBaseSha '' -RunId 123 -RunAttempt 1 `
            -Body "$script:marker`nolder failure"

        $comment.body | Should -BeExactly $newerBody
        Should -Invoke Invoke-PublicApiGitHubJson -ModuleName PublicApiComment -Times 0 -Exactly `
            -ParameterFilter { $Method -in @('POST', 'PATCH') }
    }

    It 'preserves an existing report with unknown run identity when a failed run has no artifact' {
        Mock Get-PublicApiGitHubCollection -ModuleName PublicApiComment {
            @(@{
                id = 9
                body = "$script:marker`nlegacy comment"
                user = @{ login = 'github-actions[bot]' }
            })
        }

        $comment = Publish-PublicApiPullRequestComment -Repository example/repository -PullRequest 42 `
            -HeadSha $script:sha -MergeBaseSha '' -RunId 123 -RunAttempt 1 `
            -Body "$script:marker`nnew failure"

        $comment.body | Should -BeExactly "$script:marker`nlegacy comment"
        Should -Invoke Invoke-PublicApiGitHubJson -ModuleName PublicApiComment -Times 0 -Exactly `
            -ParameterFilter { $Method -in @('POST', 'PATCH') }
    }
}

Describe 'Public API GitHub collection' {
    It 'flattens paginated comment arrays without inventing another page boundary' {
        Mock gh -ModuleName PublicApiComment {
            $global:LASTEXITCODE = 0
            return '[[{"id":1}],[{"id":2},{"id":3}]]'
        }

        $comments = InModuleScope PublicApiComment {
            @(Get-PublicApiGitHubCollection 'repos/example/repository/issues/42/comments')
        }

        @($comments.id) | Should -Be @(1, 2, 3)
    }

    It 'resolves a fork pull request from trusted head identity' {
        Mock gh -ModuleName PublicApiComment {
            $global:LASTEXITCODE = 0
            return @'
[[{
  "number": 42,
  "state": "open",
  "head": {
    "sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "repo": { "full_name": "contributor/repository" }
  }
}]]
'@
        }

        $number = Resolve-PublicApiPullRequest -Repository example/repository `
            -HeadRepository contributor/repository -HeadBranch feature -HeadSha ('a' * 40)

        $number | Should -Be 42
    }
}

Describe 'Public API publication failure handling' {
    It 'publishes a useful failure comment for an unsuccessful producer without an artifact' {
        Mock Resolve-PublicApiPullRequest -ModuleName PublicApiComment { 42 }
        Mock Publish-PublicApiPullRequestComment -ModuleName PublicApiComment {
            @{ id = 7; body = $Body }
        }

        Publish-PublicApiComment -ArtifactPath (Join-Path $TestDrive 'missing.json') `
            -Repository example/repository -HeadRepository example/fork -HeadBranch feature `
            -RunId 123 -RunAttempt 1 `
            -HeadSha ('a' * 40) -Conclusion failure

        Should -Invoke Publish-PublicApiPullRequestComment -ModuleName PublicApiComment -Times 1 -Exactly `
            -ParameterFilter { $Body -match 'did not complete' }
    }

    It 'fails the publisher after reporting an artifact contract violation' {
        Mock Resolve-PublicApiPullRequest -ModuleName PublicApiComment { 42 }
        $path = Join-Path $TestDrive 'invalid.json'
        '{' | Set-Content -LiteralPath $path -Encoding utf8
        Mock Publish-PublicApiPullRequestComment -ModuleName PublicApiComment {
            @{ id = 7; body = $Body }
        }

        {
            Publish-PublicApiComment -ArtifactPath $path `
                -Repository example/repository -HeadRepository example/fork -HeadBranch feature `
                -RunId 123 -RunAttempt 1 `
                -HeadSha ('a' * 40) -Conclusion success
        } | Should -Throw
        Should -Invoke Publish-PublicApiPullRequestComment -ModuleName PublicApiComment -Times 1 -Exactly `
            -ParameterFilter { $Body -match 'artifact was invalid' }
    }

    It 'rejects malformed workflow identity before contacting GitHub' {
        Mock Resolve-PublicApiPullRequest -ModuleName PublicApiComment { 42 }
        Mock Publish-PublicApiPullRequestComment -ModuleName PublicApiComment {}

        {
            Publish-PublicApiComment -ArtifactPath (Join-Path $TestDrive 'missing.json') `
                -Repository 'owner/repository malicious' -HeadRepository example/fork -HeadBranch feature `
                -RunId 123 -RunAttempt 1 `
                -HeadSha ('a' * 40) -Conclusion failure
        } | Should -Throw '*owner/name identifier*'
        Should -Invoke Publish-PublicApiPullRequestComment -ModuleName PublicApiComment -Times 0 -Exactly
    }
}
