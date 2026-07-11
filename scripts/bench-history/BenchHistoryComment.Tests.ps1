#Requires -Modules @{ ModuleName = 'Pester'; ModuleVersion = '5.0' }

# Pester suite for BenchHistoryComment.psm1. The one tool that would touch GitHub for real -- `gh` --
# is isolated behind the module's Invoke-GhCapture seam and mocked here in the module's scope, so the
# rolling-comment logic (find-by-marker, update-in-place vs. create, delete-if-present, error
# propagation, and the path-splice validation) is exercised without posting to a real pull request.
# Body files are real temp files so the on-disk read and the missing/empty guards are asserted, not
# faked.

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot 'BenchHistoryComment.psm1') -Force

    $script:Marker = '<!-- folo-bench-history-pr -->'

    # A real body file (already carrying the marker) so Publish-RollingComment's Test-Path guard
    # passes; `gh` is mocked, so its contents only matter where a test inspects the posted body.
    $script:BodyFile = Join-Path ([System.IO.Path]::GetTempPath()) ("bh-comment-$([guid]::NewGuid().ToString('n')).md")
    Set-Content -LiteralPath $script:BodyFile -Value "$script:Marker`n`nrendered body" -Encoding utf8

    # A body file WITHOUT the marker, to prove the module prepends it before posting.
    $script:UnmarkedBodyFile = Join-Path ([System.IO.Path]::GetTempPath()) ("bh-comment-unmarked-$([guid]::NewGuid().ToString('n')).md")
    Set-Content -LiteralPath $script:UnmarkedBodyFile -Value 'rendered body without marker' -Encoding utf8
}

AfterAll {
    Remove-Item -LiteralPath $script:BodyFile -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $script:UnmarkedBodyFile -ErrorAction SilentlyContinue
}

Describe 'Find-RollingComment (mocked gh api)' {
    Context 'when a comment carrying the marker exists' {
        BeforeEach {
            Mock gh -ModuleName BenchHistoryComment {
                $global:LASTEXITCODE = 0
                '[{"id":7,"body":"just a chat comment","html_url":"https://github.com/o/r/pull/5#issuecomment-7"},{"id":42,"body":"<!-- folo-bench-history-pr -->\nfindings","html_url":"https://github.com/o/r/pull/5#issuecomment-42"}]'
            }
        }

        It 'returns that comment' {
            $comment = Find-RollingComment -Repo 'o/r' -PrNumber '5' -Marker $script:Marker
            $comment.id | Should -Be 42
            $comment.html_url | Should -Be 'https://github.com/o/r/pull/5#issuecomment-42'
        }

        It 'lists the PR issue comments with pagination' {
            Find-RollingComment -Repo 'o/r' -PrNumber '5' -Marker $script:Marker | Out-Null
            Should -Invoke gh -ModuleName BenchHistoryComment -ParameterFilter {
                ($args -contains 'api') -and ($args -contains '--paginate') -and
                ($args -contains 'repos/o/r/issues/5/comments')
            }
        }
    }

    Context 'when no comment carries the marker' {
        BeforeEach {
            Mock gh -ModuleName BenchHistoryComment {
                $global:LASTEXITCODE = 0
                '[{"id":7,"body":"just a chat comment","html_url":"https://github.com/o/r/pull/5#issuecomment-7"}]'
            }
        }

        It 'returns null' {
            Find-RollingComment -Repo 'o/r' -PrNumber '5' -Marker $script:Marker | Should -BeNullOrEmpty
        }
    }

    Context 'when the comment list is empty' {
        BeforeEach {
            Mock gh -ModuleName BenchHistoryComment { $global:LASTEXITCODE = 0; '[]' }
        }

        It 'returns null' {
            Find-RollingComment -Repo 'o/r' -PrNumber '5' -Marker $script:Marker | Should -BeNullOrEmpty
        }
    }

    Context 'when gh fails' {
        BeforeEach {
            Mock gh -ModuleName BenchHistoryComment { $global:LASTEXITCODE = 1; 'HTTP 503: Service Unavailable' }
        }

        It 'throws, surfacing the gh output' {
            { Find-RollingComment -Repo 'o/r' -PrNumber '5' -Marker $script:Marker } | Should -Throw '*503*'
        }
    }

    Context 'when gh prints a warning to stderr but still exits 0' {
        BeforeEach {
            # The real gotcha: gh emits a note on stderr while returning valid JSON on stdout and
            # exiting 0. A naive 2>&1 merge would corrupt the JSON; the module must parse stdout only.
            Mock gh -ModuleName BenchHistoryComment {
                Write-Error 'gh: a new release of gh is available' -ErrorAction Continue
                $global:LASTEXITCODE = 0
                '[{"id":42,"body":"<!-- folo-bench-history-pr -->\nfindings","html_url":"https://github.com/o/r/pull/5#issuecomment-42"}]'
            }
        }

        It 'ignores the stderr note and still parses the comment from stdout' {
            (Find-RollingComment -Repo 'o/r' -PrNumber '5' -Marker $script:Marker).id | Should -Be 42
        }
    }

    Context 'input validation' {
        It 'rejects a malformed repository' {
            { Find-RollingComment -Repo 'not-a-repo' -PrNumber '5' -Marker $script:Marker } |
                Should -Throw "*owner/name*"
        }

        It 'rejects a non-numeric PR number' {
            { Find-RollingComment -Repo 'o/r' -PrNumber '5; rm -rf /' -Marker $script:Marker } |
                Should -Throw '*positive integer*'
        }

        It 'rejects a zero PR number' {
            { Find-RollingComment -Repo 'o/r' -PrNumber '0' -Marker $script:Marker } |
                Should -Throw '*positive integer*'
        }
    }
}

Describe 'Publish-RollingComment (mocked gh api)' {
    Context 'when a rolling comment already exists' {
        BeforeEach {
            Mock gh -ModuleName BenchHistoryComment {
                if ($args -contains 'PATCH') {
                    $global:LASTEXITCODE = 0
                    return '{"id":42,"html_url":"https://github.com/o/r/pull/5#issuecomment-42"}'
                }
                if ($args -contains 'POST') {
                    $global:LASTEXITCODE = 0
                    return '{"id":99,"html_url":"https://github.com/o/r/pull/5#issuecomment-99"}'
                }
                # list
                $global:LASTEXITCODE = 0
                return '[{"id":42,"body":"<!-- folo-bench-history-pr -->\nold findings","html_url":"https://github.com/o/r/pull/5#issuecomment-42"}]'
            }
        }

        It 'updates the existing comment instead of posting a duplicate' {
            Publish-RollingComment -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -BodyFile $script:BodyFile | Out-Null
            Should -Invoke gh -ModuleName BenchHistoryComment -ParameterFilter { $args -contains 'PATCH' }
            Should -Invoke gh -ModuleName BenchHistoryComment -ParameterFilter { $args -contains 'POST' } -Times 0 -Exactly
        }

        It 'patches the matched comment id and passes the rendered body' {
            Publish-RollingComment -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -BodyFile $script:BodyFile | Out-Null
            Should -Invoke gh -ModuleName BenchHistoryComment -ParameterFilter {
                ($args -contains 'PATCH') -and ($args -contains 'repos/o/r/issues/comments/42') -and
                (($args -join "`n") -like '*body=*rendered body*')
            }
        }

        It 'returns the existing comment url' {
            Publish-RollingComment -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -BodyFile $script:BodyFile |
                Should -Be 'https://github.com/o/r/pull/5#issuecomment-42'
        }
    }

    Context 'when no rolling comment exists yet' {
        BeforeEach {
            Mock gh -ModuleName BenchHistoryComment {
                if ($args -contains 'POST') {
                    $global:LASTEXITCODE = 0
                    return '{"id":99,"html_url":"https://github.com/o/r/pull/5#issuecomment-99"}'
                }
                $global:LASTEXITCODE = 0
                return '[]'
            }
        }

        It 'creates a new comment on the PR' {
            Publish-RollingComment -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -BodyFile $script:BodyFile | Out-Null
            Should -Invoke gh -ModuleName BenchHistoryComment -ParameterFilter {
                ($args -contains 'POST') -and ($args -contains 'repos/o/r/issues/5/comments')
            }
        }

        It 'returns the created comment url' {
            Publish-RollingComment -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -BodyFile $script:BodyFile |
                Should -Be 'https://github.com/o/r/pull/5#issuecomment-99'
        }

        It 'prepends the marker when the rendered body lacks it' {
            Publish-RollingComment -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -BodyFile $script:UnmarkedBodyFile | Out-Null
            Should -Invoke gh -ModuleName BenchHistoryComment -ParameterFilter {
                ($args -contains 'POST') -and (($args -join "`n") -like "*$($script:Marker)*")
            }
        }
    }

    Context 'error handling' {
        It 'throws when the body file does not exist' {
            $missing = Join-Path ([System.IO.Path]::GetTempPath()) "bh-missing-$([guid]::NewGuid().ToString('n')).md"
            { Publish-RollingComment -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -BodyFile $missing } |
                Should -Throw '*does not exist*'
        }

        It 'throws when the body file is empty' {
            $empty = Join-Path ([System.IO.Path]::GetTempPath()) "bh-empty-$([guid]::NewGuid().ToString('n')).md"
            Set-Content -LiteralPath $empty -Value '' -Encoding utf8
            try {
                { Publish-RollingComment -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -BodyFile $empty } |
                    Should -Throw '*is empty*'
            }
            finally {
                Remove-Item -LiteralPath $empty -ErrorAction SilentlyContinue
            }
        }

        It 'throws when gh create fails, surfacing its output' {
            Mock gh -ModuleName BenchHistoryComment {
                if ($args -contains 'POST') { $global:LASTEXITCODE = 1; return 'could not create comment' }
                $global:LASTEXITCODE = 0; return '[]'
            }
            { Publish-RollingComment -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -BodyFile $script:BodyFile } |
                Should -Throw '*could not create comment*'
        }

        It 'throws when gh edit fails, surfacing its output' {
            Mock gh -ModuleName BenchHistoryComment {
                if ($args -contains 'PATCH') { $global:LASTEXITCODE = 1; return 'could not edit comment' }
                $global:LASTEXITCODE = 0
                return '[{"id":42,"body":"<!-- folo-bench-history-pr -->\nold","html_url":"https://github.com/o/r/pull/5#issuecomment-42"}]'
            }
            { Publish-RollingComment -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -BodyFile $script:BodyFile } |
                Should -Throw '*could not edit comment*'
        }
    }
}

Describe 'Remove-RollingComment (mocked gh api)' {
    Context 'when a rolling comment exists' {
        BeforeEach {
            Mock gh -ModuleName BenchHistoryComment {
                if ($args -contains 'DELETE') { $global:LASTEXITCODE = 0; return '' }
                $global:LASTEXITCODE = 0
                return '[{"id":7,"body":"chatter","html_url":"https://github.com/o/r/pull/5#issuecomment-7"},{"id":42,"body":"<!-- folo-bench-history-pr -->\nstale findings","html_url":"https://github.com/o/r/pull/5#issuecomment-42"}]'
            }
        }

        It 'deletes the matching comment and returns true' {
            $removed = Remove-RollingComment -Repo 'o/r' -PrNumber '5' -Marker $script:Marker
            $removed | Should -BeTrue
            Should -Invoke gh -ModuleName BenchHistoryComment -ParameterFilter {
                ($args -contains 'DELETE') -and ($args -contains 'repos/o/r/issues/comments/42')
            }
        }

        It 'never deletes a non-matching comment' {
            Remove-RollingComment -Repo 'o/r' -PrNumber '5' -Marker $script:Marker | Out-Null
            Should -Invoke gh -ModuleName BenchHistoryComment -ParameterFilter {
                ($args -contains 'DELETE') -and ($args -contains 'repos/o/r/issues/comments/7')
            } -Times 0 -Exactly
        }
    }

    Context 'when no rolling comment exists' {
        BeforeEach {
            Mock gh -ModuleName BenchHistoryComment { $global:LASTEXITCODE = 0; '[]' }
        }

        It 'deletes nothing and returns false' {
            $removed = Remove-RollingComment -Repo 'o/r' -PrNumber '5' -Marker $script:Marker
            $removed | Should -BeFalse
            Should -Invoke gh -ModuleName BenchHistoryComment -ParameterFilter { $args -contains 'DELETE' } -Times 0 -Exactly
        }
    }

    Context 'when gh delete fails' {
        BeforeEach {
            Mock gh -ModuleName BenchHistoryComment {
                if ($args -contains 'DELETE') { $global:LASTEXITCODE = 1; return 'could not delete comment' }
                $global:LASTEXITCODE = 0
                return '[{"id":42,"body":"<!-- folo-bench-history-pr -->\nstale","html_url":"https://github.com/o/r/pull/5#issuecomment-42"}]'
            }
        }

        It 'throws, surfacing the gh output' {
            { Remove-RollingComment -Repo 'o/r' -PrNumber '5' -Marker $script:Marker } |
                Should -Throw '*could not delete comment*'
        }
    }
}
