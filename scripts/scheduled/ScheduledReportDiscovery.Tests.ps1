#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }
# Exercises the report publisher's open-title search and current-issue boundary in memory.
# Fake GitHub pages protect against broad discovery, partial results and stale search hits;
# no test reads issue content from GitHub or creates files.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'
Import-Module (Join-Path $PSScriptRoot 'ScheduledGitHub.psm1') -Force

Describe 'Open title-prefix report discovery' {
    InModuleScope ScheduledGitHub {
        BeforeEach {
            $script:attemptUrl = 'https://github.com/example/repo/actions/runs/10/attempts/1'
            $script:issue = @{
                number = 42; state = 'open'; title = 'Scheduled validation failed on 2026-09-11'
                body = $script:attemptUrl; comments = 0
            }
            $script:liveIssues = @{ 42L = $script:issue }
            $script:pages = @(@{
                items = @($script:issue.Clone()); total_count = 1; incomplete_results = $false
            })
            Mock Invoke-ScheduledGitHubJson {
                param($Endpoint, $Method)
                if ($Method -in @('POST', 'PATCH')) { throw "Unexpected write: $Endpoint" }
                switch -Regex ($Endpoint) {
                    '^search/issues\?.*&page=(\d+)$' { return $script:pages[[int]$Matches[1] - 1] }
                    '^repos/example/repo/issues/(\d+)$' { return $script:liveIssues[[long]$Matches[1]] }
                    default { throw "Unexpected request: $Endpoint" }
                }
            }
        }

        It 'uses only repository-scoped open title search and refreshes the matching issue' {
            $reports = @(Get-ScheduledReport example/repo $script:attemptUrl)
            $reports.Count | Should -Be 1
            $reports[0].number | Should -Be 42
            Should -Invoke Invoke-ScheduledGitHubJson -Times 1 -Exactly -ParameterFilter {
                [Uri]::UnescapeDataString($Endpoint) -ceq 'search/issues?q=repo:example/repo is:issue is:open in:title "Scheduled validation failed on"&sort=created&order=asc&per_page=100&page=1'
            }
            Should -Invoke Invoke-ScheduledGitHubJson -Times 1 -Exactly -ParameterFilter {
                $Endpoint -ceq 'repos/example/repo/issues/42'
            }
            Should -Invoke Invoke-ScheduledGitHubJson -Times 2 -Exactly
        }

        It 'accepts a human title suffix without parsing it as a date' {
            $script:pages[0].items[0].title = 'Scheduled validation failed on the nightly run'
            $script:issue.title = $script:pages[0].items[0].title
            @(Get-ScheduledReport example/repo $script:attemptUrl).Count | Should -Be 1
        }

        It 'ignores a <Case> title before fetching content or discussion' -ForEach @(
            @{ Case = 'problem'; Title = 'Fix the failing Miri check' }
            @{ Case = 'non-prefix phrase'; Title = 'Investigate: Scheduled validation failed on 2026-09-11' }
            @{ Case = 'case-variant prefix'; Title = 'scheduled validation failed on 2026-09-11' }
            @{ Case = 'missing space boundary'; Title = 'Scheduled validation failed on-call' }
            @{ Case = 'prefix without its trailing space'; Title = 'Scheduled validation failed on' }
        ) {
            $script:pages[0].items[0].title = $Title
            $script:pages[0].items[0].labels = @('scheduled-run-failure')
            $script:pages[0].items[0].comments = 1
            @(Get-ScheduledReport example/repo $script:attemptUrl).Count | Should -Be 0
            Should -Invoke Invoke-ScheduledGitHubJson -Times 1 -Exactly
        }

        It 'ignores <Case> search hits before refreshing them' -ForEach @(
            @{ Case = 'closed'; Patch = @{ state = 'closed' } }
            @{ Case = 'pull request'; Patch = @{ pull_request = @{ url = 'https://api.github.com/repos/example/repo/pulls/42' } } }
        ) {
            foreach ($key in $Patch.Keys) { $script:pages[0].items[0][$key] = $Patch[$key] }
            @(Get-ScheduledReport example/repo $script:attemptUrl).Count | Should -Be 0
            Should -Invoke Invoke-ScheduledGitHubJson -Times 1 -Exactly
        }

        It 'rejects a matching search hit whose current issue is <Case>' -ForEach @(
            @{ Case = 'closed'; State = 'closed'; Title = 'Scheduled validation failed on 2026-09-11' }
            @{ Case = 'retitled'; State = 'open'; Title = 'A differently named report' }
        ) {
            $script:issue.state = $State
            $script:issue.title = $Title
            $script:issue.comments = 1
            @(Get-ScheduledReport example/repo $script:attemptUrl).Count | Should -Be 0
            Should -Invoke Invoke-ScheduledGitHubJson -Times 2 -Exactly
        }

        It 'propagates failure to refresh a candidate rather than trusting indexed content' {
            Mock Invoke-ScheduledGitHubJson { throw [IO.IOException]::new() } -ParameterFilter {
                $Endpoint -ceq 'repos/example/repo/issues/42'
            }
            { Get-ScheduledReport example/repo $script:attemptUrl } | Should -Throw
        }

        It 'accepts a complete empty search without additional queries' {
            $script:pages[0] = @{ items = @(); total_count = 0; incomplete_results = $false }
            @(Get-ScheduledReport example/repo $script:attemptUrl).Count | Should -Be 0
            Should -Invoke Invoke-ScheduledGitHubJson -Times 1 -Exactly
        }

        It 'deduplicates and orders matching issues independently of search order' {
            $earlier = $script:issue.Clone()
            $earlier.number = 41
            $script:liveIssues[41L] = $earlier
            $script:pages[0].items = @($script:issue.Clone(), $earlier.Clone(), $script:issue.Clone())
            $script:pages[0].total_count = 3
            $reports = @(Get-ScheduledReport example/repo $script:attemptUrl)
            $reports.number | Should -Be @(41, 42)
            Should -Invoke Invoke-ScheduledGitHubJson -Times 1 -Exactly -ParameterFilter {
                $Endpoint -ceq 'repos/example/repo/issues/42'
            }
        }

        It 'finds a report on the last accessible page for <Count> search results' -ForEach @(
            # Exercise a page boundary and the API's full accessible result limit.
            @{ Count = 101 }, @{ Count = 1000 }
        ) {
            $script:pages = @(for ($offset = 0; $offset -lt $Count; $offset += 100) {
                $items = @(for ($index = $offset; $index -lt [Math]::Min($offset + 100, $Count); $index++) {
                    if ($index -eq $Count - 1) { $script:issue.Clone() }
                    else {
                        @{ number = 100 + $index; state = 'open'; title = 'Other: Scheduled validation failed on a run' }
                    }
                })
                @{ items = $items; total_count = $Count; incomplete_results = $false }
            })
            $reports = @(Get-ScheduledReport example/repo $script:attemptUrl)
            $reports.Count | Should -Be 1
            $reports[0].number | Should -Be 42
            Should -Invoke Invoke-ScheduledGitHubJson -Times ($script:pages.Count + 1) -Exactly
        }

        It 'rejects <Case> search results without refreshing any candidates' -ForEach @(
            @{ Case = 'incomplete'; Patch = @{ incomplete_results = $true } }
            @{ Case = 'over the search cap'; Patch = @{ total_count = 1001 } }
            @{ Case = 'short final page'; Patch = @{ total_count = 2 } }
            @{ Case = 'missing items'; Patch = @{ items = $null } }
            @{ Case = 'missing completeness flag'; Patch = @{ incomplete_results = $null } }
            @{ Case = 'invalid completeness flag'; Patch = @{ incomplete_results = 'false' } }
            @{ Case = 'missing total'; Patch = @{ total_count = $null } }
            @{ Case = 'invalid total'; Patch = @{ total_count = '1' } }
            @{ Case = 'negative total'; Patch = @{ total_count = -1 } }
        ) {
            foreach ($key in $Patch.Keys) { $script:pages[0][$key] = $Patch[$key] }
            { Get-ScheduledReport example/repo $script:attemptUrl } | Should -Throw
            Should -Invoke Invoke-ScheduledGitHubJson -Times 1 -Exactly
        }

        It 'does not use first-page matches when a later page is <Case>' -ForEach @(
            @{ Case = 'unavailable' }, @{ Case = 'incomplete' }, @{ Case = 'empty' }
        ) {
            $script:pages[0].items = @(1..100 | ForEach-Object { $script:issue.Clone() })
            $script:pages[0].total_count = 101
            if ($Case -eq 'unavailable') {
                Mock Invoke-ScheduledGitHubJson { throw [IO.IOException]::new() } -ParameterFilter {
                    $Endpoint -match '^search/issues\?.*&page=2$'
                }
            } else {
                $script:pages += @{
                    items = @(); total_count = 101; incomplete_results = ($Case -eq 'incomplete')
                }
            }
            { Get-ScheduledReport example/repo $script:attemptUrl } | Should -Throw
            Should -Invoke Invoke-ScheduledGitHubJson -Times 0 -Exactly -ParameterFilter {
                $Endpoint -match '^repos/'
            }
        }
    }
}
