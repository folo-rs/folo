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
    Remove-Item -LiteralPath (Join-Path ([System.IO.Path]::GetTempPath()) 'bh-comment-captured-body.md') -ErrorAction SilentlyContinue
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

        It 'patches the matched comment id and sends the rendered body via a file' {
            Publish-RollingComment -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -BodyFile $script:BodyFile | Out-Null
            Should -Invoke gh -ModuleName BenchHistoryComment -ParameterFilter {
                ($args -contains 'PATCH') -and ($args -contains 'repos/o/r/issues/comments/42') -and
                ($args -contains "body=@$($script:BodyFile)")
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
                # Capture the body file's content while it still exists: Publish-RollingComment sends
                # the body via `-F body=@<path>` and deletes any temp file it created once `gh`
                # returns, so a post-hoc ParameterFilter could no longer read it. The fixed capture
                # path is recomputed identically in the assertion, so no cross-scope variable is
                # needed between this module-scoped mock and the test.
                foreach ($a in $args) {
                    if ($a -like 'body=@*') {
                        Copy-Item -LiteralPath $a.Substring('body=@'.Length) `
                            -Destination (Join-Path ([System.IO.Path]::GetTempPath()) 'bh-comment-captured-body.md') -Force
                    }
                }
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

        It 'prepends the marker when the rendered body lacks it, sending it via a file' {
            Publish-RollingComment -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -BodyFile $script:UnmarkedBodyFile | Out-Null
            $sent = Get-Content -LiteralPath (Join-Path ([System.IO.Path]::GetTempPath()) 'bh-comment-captured-body.md') -Raw
            $sent | Should -BeLike "*$($script:Marker)*"
            $sent | Should -BeLike '*rendered body without marker*'
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

Describe 'Get-CommitsBehind (mocked gh compare api)' {
    BeforeAll {
        # Full 40-hex SHAs: Get-CommitsBehind validates the shape before splicing them into the
        # compare path, so short placeholders would be rejected.
        $script:BaseSha = '1111111111111111111111111111111111111111'
        $script:HeadSha = '2222222222222222222222222222222222222222'
    }

    Context 'when the two commits share history' {
        BeforeEach {
            Mock gh -ModuleName BenchHistoryComment {
                $global:LASTEXITCODE = 0
                '{"status":"ahead","ahead_by":3,"behind_by":0,"total_commits":3}'
            }
        }

        It 'returns the ahead-by count as the distance' {
            $result = Get-CommitsBehind -Repo 'o/r' -BaseSha $script:BaseSha -HeadSha $script:HeadSha
            $result.Related | Should -BeTrue
            $result.Behind | Should -Be 3
        }

        It 'queries the compare endpoint for base...head' {
            Get-CommitsBehind -Repo 'o/r' -BaseSha $script:BaseSha -HeadSha $script:HeadSha | Out-Null
            Should -Invoke gh -ModuleName BenchHistoryComment -ParameterFilter {
                ($args -contains 'api') -and ($args -contains "repos/o/r/compare/$($script:BaseSha)...$($script:HeadSha)")
            }
        }
    }

    Context 'when the head is identical to the base' {
        BeforeEach {
            Mock gh -ModuleName BenchHistoryComment { $global:LASTEXITCODE = 0; '{"status":"identical","ahead_by":0,"behind_by":0}' }
        }

        It 'reports a related zero distance' {
            $result = Get-CommitsBehind -Repo 'o/r' -BaseSha $script:BaseSha -HeadSha $script:HeadSha
            $result.Related | Should -BeTrue
            $result.Behind | Should -Be 0
        }
    }

    Context 'when the commits share no history' {
        BeforeEach {
            # The compare endpoint 404s with this message for unrelated histories (e.g. a force-push).
            Mock gh -ModuleName BenchHistoryComment { $global:LASTEXITCODE = 1; 'gh: No common ancestor for the two commits (HTTP 404)' }
        }

        It 'reports unrelated with no distance' {
            $result = Get-CommitsBehind -Repo 'o/r' -BaseSha $script:BaseSha -HeadSha $script:HeadSha
            $result.Related | Should -BeFalse
            $result.Behind | Should -Be 0
        }
    }

    Context 'when the compare fails for another reason' {
        BeforeEach {
            Mock gh -ModuleName BenchHistoryComment { $global:LASTEXITCODE = 1; 'HTTP 500: Internal Server Error' }
        }

        It 'rethrows rather than reporting a bogus "out of date"' {
            { Get-CommitsBehind -Repo 'o/r' -BaseSha $script:BaseSha -HeadSha $script:HeadSha } | Should -Throw '*HTTP 500*'
        }
    }

    Context 'input validation' {
        It 'rejects a non-hex base SHA' {
            { Get-CommitsBehind -Repo 'o/r' -BaseSha 'not-a-sha' -HeadSha $script:HeadSha } | Should -Throw '*40-character hex*'
        }

        It 'rejects a malformed repository' {
            { Get-CommitsBehind -Repo 'bad' -BaseSha $script:BaseSha -HeadSha $script:HeadSha } | Should -Throw '*owner/name*'
        }
    }
}

Describe 'Set-RollingCommentStaleness (mocked gh api)' {
    BeforeAll {
        $script:CommitPrefix = '<!-- folo-bench-history-commit:'
        $script:AnalyzedSha = '1111111111111111111111111111111111111111'
        $script:HeadSha2 = '2222222222222222222222222222222222222222'
        # Fixed capture path (recomputed identically in the mock and the assertions) for the PATCHed
        # body, mirroring the Publish-RollingComment tests: the temp body file is deleted once `gh`
        # returns, so a post-hoc ParameterFilter could not read it.
        $script:StaleCapture = Join-Path ([System.IO.Path]::GetTempPath()) 'bh-stale-captured-body.md'
    }

    AfterAll {
        Remove-Item -LiteralPath $script:StaleCapture -ErrorAction SilentlyContinue
    }

    Context 'when the analyzed commit is behind head' {
        BeforeEach {
            Remove-Item -LiteralPath $script:StaleCapture -ErrorAction SilentlyContinue
            Mock gh -ModuleName BenchHistoryComment {
                foreach ($a in $args) {
                    if ($a -like 'body=@*') {
                        Copy-Item -LiteralPath $a.Substring('body=@'.Length) `
                            -Destination (Join-Path ([System.IO.Path]::GetTempPath()) 'bh-stale-captured-body.md') -Force
                    }
                }
                if ($args -contains 'PATCH') { $global:LASTEXITCODE = 0; return '{"id":42,"html_url":"https://github.com/o/r/pull/5#issuecomment-42"}' }
                foreach ($a in $args) { if ($a -like 'repos/*/compare/*') { $global:LASTEXITCODE = 0; return '{"status":"ahead","ahead_by":3}' } }
                $global:LASTEXITCODE = 0
                return '[{"id":42,"body":"<!-- folo-bench-history-pr -->\n<!-- folo-bench-history-commit:1111111111111111111111111111111111111111 -->\n\n### Performance impact\nold findings","html_url":"https://github.com/o/r/pull/5#issuecomment-42"}]'
            }
        }

        It 'patches the comment with an N-commits-behind warning and preserves the analyzed marker' {
            $result = Set-RollingCommentStaleness -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -CommitMarkerPrefix $script:CommitPrefix -HeadSha $script:HeadSha2
            $result | Should -BeTrue
            Should -Invoke gh -ModuleName BenchHistoryComment -ParameterFilter {
                ($args -contains 'PATCH') -and ($args -contains 'repos/o/r/issues/comments/42')
            }
            $sent = Get-Content -LiteralPath $script:StaleCapture -Raw
            $sent | Should -BeLike '*3 commits behind HEAD*'
            $sent | Should -BeLike '*[!WARNING]*'
            # The analyzed-commit marker must survive so the next run can still parse it.
            $sent | Should -BeLike "*$($script:CommitPrefix)$($script:AnalyzedSha)*"
        }
    }

    Context 'when the analyzed commit is exactly one commit behind' {
        BeforeEach {
            Remove-Item -LiteralPath $script:StaleCapture -ErrorAction SilentlyContinue
            Mock gh -ModuleName BenchHistoryComment {
                foreach ($a in $args) {
                    if ($a -like 'body=@*') {
                        Copy-Item -LiteralPath $a.Substring('body=@'.Length) `
                            -Destination (Join-Path ([System.IO.Path]::GetTempPath()) 'bh-stale-captured-body.md') -Force
                    }
                }
                if ($args -contains 'PATCH') { $global:LASTEXITCODE = 0; return '{"id":42}' }
                foreach ($a in $args) { if ($a -like 'repos/*/compare/*') { $global:LASTEXITCODE = 0; return '{"status":"ahead","ahead_by":1}' } }
                $global:LASTEXITCODE = 0
                return '[{"id":42,"body":"<!-- folo-bench-history-pr -->\n<!-- folo-bench-history-commit:1111111111111111111111111111111111111111 -->\n\nfindings","html_url":"u"}]'
            }
        }

        It 'uses the singular "commit"' {
            Set-RollingCommentStaleness -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -CommitMarkerPrefix $script:CommitPrefix -HeadSha $script:HeadSha2 | Out-Null
            $sent = Get-Content -LiteralPath $script:StaleCapture -Raw
            $sent | Should -BeLike '*1 commit behind HEAD*'
            $sent | Should -Not -BeLike '*1 commits behind*'
        }
    }

    Context 'when the analyzed commit shares no history with head' {
        BeforeEach {
            Remove-Item -LiteralPath $script:StaleCapture -ErrorAction SilentlyContinue
            Mock gh -ModuleName BenchHistoryComment {
                foreach ($a in $args) {
                    if ($a -like 'body=@*') {
                        Copy-Item -LiteralPath $a.Substring('body=@'.Length) `
                            -Destination (Join-Path ([System.IO.Path]::GetTempPath()) 'bh-stale-captured-body.md') -Force
                    }
                }
                if ($args -contains 'PATCH') { $global:LASTEXITCODE = 0; return '{"id":42}' }
                foreach ($a in $args) { if ($a -like 'repos/*/compare/*') { $global:LASTEXITCODE = 1; return 'gh: No common ancestor for the two commits (HTTP 404)' } }
                $global:LASTEXITCODE = 0
                return '[{"id":42,"body":"<!-- folo-bench-history-pr -->\n<!-- folo-bench-history-commit:1111111111111111111111111111111111111111 -->\n\nfindings","html_url":"u"}]'
            }
        }

        It 'patches a numberless "out of date" warning' {
            Set-RollingCommentStaleness -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -CommitMarkerPrefix $script:CommitPrefix -HeadSha $script:HeadSha2 | Out-Null
            $sent = Get-Content -LiteralPath $script:StaleCapture -Raw
            $sent | Should -BeLike '*out of date*'
            $sent | Should -Not -BeLike '*behind HEAD*'
        }
    }

    Context 'when the compare lookup fails for a transient reason' {
        BeforeEach {
            Remove-Item -LiteralPath $script:StaleCapture -ErrorAction SilentlyContinue
            Mock gh -ModuleName BenchHistoryComment {
                foreach ($a in $args) {
                    if ($a -like 'body=@*') {
                        Copy-Item -LiteralPath $a.Substring('body=@'.Length) `
                            -Destination (Join-Path ([System.IO.Path]::GetTempPath()) 'bh-stale-captured-body.md') -Force
                    }
                }
                if ($args -contains 'PATCH') { $global:LASTEXITCODE = 0; return '{"id":42}' }
                # A non-404 compare failure (e.g. a transient 500) is a genuine error Get-CommitsBehind
                # rethrows; staleness marking is best-effort and must degrade rather than fail the run.
                foreach ($a in $args) { if ($a -like 'repos/*/compare/*') { $global:LASTEXITCODE = 1; return 'HTTP 500: Internal Server Error' } }
                $global:LASTEXITCODE = 0
                return '[{"id":42,"body":"<!-- folo-bench-history-pr -->\n<!-- folo-bench-history-commit:1111111111111111111111111111111111111111 -->\n\nfindings","html_url":"u"}]'
            }
        }

        It 'degrades to the numberless "out of date" warning instead of throwing' {
            $result = Set-RollingCommentStaleness -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -CommitMarkerPrefix $script:CommitPrefix -HeadSha $script:HeadSha2
            $result | Should -BeTrue
            $sent = Get-Content -LiteralPath $script:StaleCapture -Raw
            $sent | Should -BeLike '*out of date*'
            $sent | Should -Not -BeLike '*behind HEAD*'
        }
    }

    Context 'when the comment lacks an analyzed-commit marker (pre-change comment)' {
        BeforeEach {
            Remove-Item -LiteralPath $script:StaleCapture -ErrorAction SilentlyContinue
            Mock gh -ModuleName BenchHistoryComment {
                foreach ($a in $args) {
                    if ($a -like 'body=@*') {
                        Copy-Item -LiteralPath $a.Substring('body=@'.Length) `
                            -Destination (Join-Path ([System.IO.Path]::GetTempPath()) 'bh-stale-captured-body.md') -Force
                    }
                }
                if ($args -contains 'PATCH') { $global:LASTEXITCODE = 0; return '{"id":42}' }
                $global:LASTEXITCODE = 0
                return '[{"id":42,"body":"<!-- folo-bench-history-pr -->\n\nfindings","html_url":"u"}]'
            }
        }

        It 'falls back to "out of date" without calling the compare api' {
            Set-RollingCommentStaleness -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -CommitMarkerPrefix $script:CommitPrefix -HeadSha $script:HeadSha2 | Out-Null
            $sent = Get-Content -LiteralPath $script:StaleCapture -Raw
            $sent | Should -BeLike '*out of date*'
            Should -Invoke gh -ModuleName BenchHistoryComment -ParameterFilter { [bool]($args | Where-Object { $_ -like 'repos/*/compare/*' }) } -Times 0 -Exactly
        }
    }

    Context 'when the analyzed commit already equals head' {
        BeforeEach {
            Mock gh -ModuleName BenchHistoryComment {
                if ($args -contains 'PATCH') { $global:LASTEXITCODE = 0; return '{"id":42}' }
                foreach ($a in $args) { if ($a -like 'repos/*/compare/*') { $global:LASTEXITCODE = 0; return '{"status":"identical","ahead_by":0}' } }
                $global:LASTEXITCODE = 0
                return '[{"id":42,"body":"<!-- folo-bench-history-pr -->\n<!-- folo-bench-history-commit:1111111111111111111111111111111111111111 -->\n\nfindings","html_url":"u"}]'
            }
        }

        It 'adds no warning and does not patch' {
            $result = Set-RollingCommentStaleness -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -CommitMarkerPrefix $script:CommitPrefix -HeadSha $script:HeadSha2
            $result | Should -BeFalse
            Should -Invoke gh -ModuleName BenchHistoryComment -ParameterFilter { $args -contains 'PATCH' } -Times 0 -Exactly
        }
    }

    Context 'when the PR has no rolling comment yet' {
        BeforeEach {
            Mock gh -ModuleName BenchHistoryComment { $global:LASTEXITCODE = 0; '[]' }
        }

        It 'is a no-op returning false' {
            $result = Set-RollingCommentStaleness -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -CommitMarkerPrefix $script:CommitPrefix -HeadSha $script:HeadSha2
            $result | Should -BeFalse
            Should -Invoke gh -ModuleName BenchHistoryComment -ParameterFilter { $args -contains 'PATCH' } -Times 0 -Exactly
        }
    }

    Context 'when the comment is an in-progress placeholder (no results yet)' {
        BeforeEach {
            # A placeholder carries the dedup marker plus the in-progress marker but NO analyzed-commit
            # marker: there are no results to age, so the staleness pass must step aside entirely -
            # neither hitting the compare API nor PATCHing a misleading "out of date" banner over a
            # comment that already reads "benchmarking in progress".
            Mock gh -ModuleName BenchHistoryComment {
                if ($args -contains 'PATCH') { $global:LASTEXITCODE = 0; return '{"id":42}' }
                $global:LASTEXITCODE = 0
                return '[{"id":42,"body":"<!-- folo-bench-history-pr -->\n<!-- folo-bench-history-in-progress -->\n\n### Performance impact (vs `main`)\n\nbenchmarking in progress","html_url":"u"}]'
            }
        }

        It 'is a no-op returning false, touching neither the compare api nor a PATCH' {
            $result = Set-RollingCommentStaleness -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -CommitMarkerPrefix $script:CommitPrefix -HeadSha $script:HeadSha2
            $result | Should -BeFalse
            Should -Invoke gh -ModuleName BenchHistoryComment -ParameterFilter { $args -contains 'PATCH' } -Times 0 -Exactly
            Should -Invoke gh -ModuleName BenchHistoryComment -ParameterFilter { [bool]($args | Where-Object { $_ -like 'repos/*/compare/*' }) } -Times 0 -Exactly
        }
    }

    Context 'when the comment already carries a stale banner (re-run)' {
        BeforeEach {
            Remove-Item -LiteralPath $script:StaleCapture -ErrorAction SilentlyContinue
            Mock gh -ModuleName BenchHistoryComment {
                foreach ($a in $args) {
                    if ($a -like 'body=@*') {
                        Copy-Item -LiteralPath $a.Substring('body=@'.Length) `
                            -Destination (Join-Path ([System.IO.Path]::GetTempPath()) 'bh-stale-captured-body.md') -Force
                    }
                }
                if ($args -contains 'PATCH') { $global:LASTEXITCODE = 0; return '{"id":42}' }
                foreach ($a in $args) { if ($a -like 'repos/*/compare/*') { $global:LASTEXITCODE = 0; return '{"status":"ahead","ahead_by":3}' } }
                $global:LASTEXITCODE = 0
                return '[{"id":42,"body":"<!-- folo-bench-history-pr -->\n\n<!-- folo-bench-history-stale -->\n> [!WARNING]\n> Benchmark results are 2 commits behind HEAD. This comment will be updated when newer results are available.\n<!-- /folo-bench-history-stale -->\n\n<!-- folo-bench-history-commit:1111111111111111111111111111111111111111 -->\n\nfindings","html_url":"u"}]'
            }
        }

        It 'replaces the old banner instead of stacking a second one' {
            Set-RollingCommentStaleness -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -CommitMarkerPrefix $script:CommitPrefix -HeadSha $script:HeadSha2 | Out-Null
            $sent = Get-Content -LiteralPath $script:StaleCapture -Raw
            $sent | Should -BeLike '*3 commits behind HEAD*'
            $sent | Should -Not -BeLike '*2 commits behind HEAD*'
            ([regex]::Matches($sent, '\[!WARNING\]')).Count | Should -Be 1
        }
    }

    Context 'when -WhatIf is passed' {
        BeforeEach {
            Mock gh -ModuleName BenchHistoryComment {
                foreach ($a in $args) { if ($a -like 'repos/*/compare/*') { $global:LASTEXITCODE = 0; return '{"status":"ahead","ahead_by":3}' } }
                $global:LASTEXITCODE = 0
                return '[{"id":42,"body":"<!-- folo-bench-history-pr -->\n<!-- folo-bench-history-commit:1111111111111111111111111111111111111111 -->\n\nfindings","html_url":"u"}]'
            }
        }

        It 'reports the edit without performing the PATCH' {
            Set-RollingCommentStaleness -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -CommitMarkerPrefix $script:CommitPrefix -HeadSha $script:HeadSha2 -WhatIf | Out-Null
            Should -Invoke gh -ModuleName BenchHistoryComment -ParameterFilter { $args -contains 'PATCH' } -Times 0 -Exactly
        }
    }

    Context 'input validation' {
        It 'rejects a non-hex head SHA' {
            { Set-RollingCommentStaleness -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -CommitMarkerPrefix $script:CommitPrefix -HeadSha 'nope' } |
                Should -Throw '*40-character hex*'
        }
    }
}

Describe 'Add-StalenessBanner (unexported string transform)' {
    Context 'when an opening sentinel has no matching close (manually edited/truncated comment)' {
        It 'preserves the trailing body instead of dropping it' {
            InModuleScope BenchHistoryComment {
                $marker = '<!-- folo-bench-history-pr -->'
                # An open sentinel with NO closing sentinel is not a block we produced. Stripping from it
                # to end-of-body would silently delete the benchmark findings that follow, so they must
                # survive verbatim.
                $body = @(
                    $marker
                    ''
                    $script:StaleBannerOpen
                    '> [!WARNING]'
                    '> a banner that was never closed'
                    ''
                    '### Performance impact'
                    'precious findings that must not vanish'
                ) -join "`n"

                $result = Add-StalenessBanner -Body $body -Warning 'fresh warning' -Marker $marker

                $result | Should -BeLike '*### Performance impact*'
                $result | Should -BeLike '*precious findings that must not vanish*'
                # The fresh banner is still inserted after the marker.
                $result | Should -BeLike '*fresh warning*'
            }
        }
    }

    Context 'when the marker text also appears quoted elsewhere (e.g. inside a code block)' {
        It 'inserts after the whole-line marker, not the quoted occurrence' {
            InModuleScope BenchHistoryComment {
                $marker = '<!-- folo-bench-history-pr -->'
                # The marker text appears first inside a fenced code block (indented, with trailing text)
                # and only later on its own line as the real marker. A substring match would latch onto
                # the quoted line; a whole-line (trimmed-equality) match lands on the real marker.
                $body = @(
                    '### Example usage'
                    '```'
                    "    $marker  <- quoted in docs, NOT the real marker"
                    '```'
                    $marker
                    'findings'
                ) -join "`n"

                $result = Add-StalenessBanner -Body $body -Warning 'fresh warning' -Marker $marker
                $lines = $result -split "`n"

                ([regex]::Matches($result, '\[!WARNING\]')).Count | Should -Be 1
                $bannerLine = [array]::IndexOf($lines, '> [!WARNING]')
                $realMarkerLine = [array]::IndexOf($lines, $marker)
                # The banner must land after the real marker line, i.e. past the whole code block.
                $bannerLine | Should -BeGreaterThan $realMarkerLine
            }
        }
    }
}

Describe 'Publish-InProgressComment (mocked gh api)' {
    BeforeAll {
        # An unsorted, duplicate-free-after-sort package list so the assertions also prove the rendering
        # sorts it; kept in sync with the InModuleScope literal used by the idempotency context below.
        $script:Packages = 'pool events'
        # Fixed capture path (recomputed identically in the mock and the assertions) for the posted body:
        # the temp body file is deleted once `gh` returns, so a post-hoc ParameterFilter could not read it.
        $script:InProgressCapture = Join-Path ([System.IO.Path]::GetTempPath()) 'bh-inprogress-captured-body.md'
        # The JSON list `gh` "returns" for the already-matches case is written to a file the mock reads
        # back inline, because a mock scriptblock cannot see test-scope variables.
        $script:InProgressListFile = Join-Path ([System.IO.Path]::GetTempPath()) 'bh-inprogress-existing-list.json'
    }

    AfterAll {
        Remove-Item -LiteralPath $script:InProgressCapture -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $script:InProgressListFile -ErrorAction SilentlyContinue
    }

    Context 'when the PR has no rolling comment yet' {
        BeforeEach {
            Remove-Item -LiteralPath $script:InProgressCapture -ErrorAction SilentlyContinue
            Mock gh -ModuleName BenchHistoryComment {
                foreach ($a in $args) {
                    if ($a -like 'body=@*') {
                        Copy-Item -LiteralPath $a.Substring('body=@'.Length) `
                            -Destination (Join-Path ([System.IO.Path]::GetTempPath()) 'bh-inprogress-captured-body.md') -Force
                    }
                }
                if ($args -contains 'POST') { $global:LASTEXITCODE = 0; return '{"id":99,"html_url":"https://github.com/o/r/pull/5#issuecomment-99"}' }
                $global:LASTEXITCODE = 0
                return '[]'
            }
        }

        It 'posts a placeholder carrying both markers and the disclosed scope' {
            $result = Publish-InProgressComment -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -Packages $script:Packages
            $result | Should -BeTrue
            Should -Invoke gh -ModuleName BenchHistoryComment -ParameterFilter {
                ($args -contains 'POST') -and ($args -contains 'repos/o/r/issues/5/comments')
            }
            $sent = Get-Content -LiteralPath $script:InProgressCapture -Raw
            $sent | Should -BeLike '*<!-- folo-bench-history-pr -->*'
            $sent | Should -BeLike '*<!-- folo-bench-history-in-progress -->*'
            $sent | Should -BeLike '*Benchmarking in progress*'
            $sent | Should -BeLike '*Collection scope*'
            $sent | Should -BeLike '*events*'
            $sent | Should -BeLike '*pool*'
        }

        It 'never carries an analyzed-commit marker, so a later mark-stale treats it as in-progress' {
            Publish-InProgressComment -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -Packages $script:Packages | Out-Null
            $sent = Get-Content -LiteralPath $script:InProgressCapture -Raw
            $sent | Should -Not -BeLike '*folo-bench-history-commit:*'
        }
    }

    Context 'when a completed analyze already posted real results' {
        BeforeEach {
            Remove-Item -LiteralPath $script:InProgressCapture -ErrorAction SilentlyContinue
            # A results comment carries the dedup + analyzed-commit markers but NOT the in-progress marker;
            # the placeholder path must never overwrite those findings (no POST, no PATCH).
            Mock gh -ModuleName BenchHistoryComment {
                foreach ($a in $args) {
                    if ($a -like 'body=@*') {
                        Copy-Item -LiteralPath $a.Substring('body=@'.Length) `
                            -Destination (Join-Path ([System.IO.Path]::GetTempPath()) 'bh-inprogress-captured-body.md') -Force
                    }
                }
                $global:LASTEXITCODE = 0
                return '[{"id":42,"body":"<!-- folo-bench-history-pr -->\n<!-- folo-bench-history-commit:1111111111111111111111111111111111111111 -->\n\nreal findings","html_url":"u"}]'
            }
        }

        It 'leaves the results comment untouched, returning false' {
            $result = Publish-InProgressComment -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -Packages $script:Packages
            $result | Should -BeFalse
            Should -Invoke gh -ModuleName BenchHistoryComment -ParameterFilter { $args -contains 'POST' } -Times 0 -Exactly
            Should -Invoke gh -ModuleName BenchHistoryComment -ParameterFilter { $args -contains 'PATCH' } -Times 0 -Exactly
        }
    }

    Context 'when an in-progress placeholder already exists with a stale scope' {
        BeforeEach {
            Remove-Item -LiteralPath $script:InProgressCapture -ErrorAction SilentlyContinue
            # The placeholder carries the in-progress marker but its body differs from the freshly-rendered
            # one, so it must be refreshed (PATCHed) in place rather than left as-is.
            Mock gh -ModuleName BenchHistoryComment {
                foreach ($a in $args) {
                    if ($a -like 'body=@*') {
                        Copy-Item -LiteralPath $a.Substring('body=@'.Length) `
                            -Destination (Join-Path ([System.IO.Path]::GetTempPath()) 'bh-inprogress-captured-body.md') -Force
                    }
                }
                if ($args -contains 'PATCH') { $global:LASTEXITCODE = 0; return '{"id":42}' }
                $global:LASTEXITCODE = 0
                return '[{"id":42,"body":"<!-- folo-bench-history-pr -->\n<!-- folo-bench-history-in-progress -->\n\nan out-of-date placeholder scope","html_url":"u"}]'
            }
        }

        It 'refreshes the placeholder in place with the current scope' {
            $result = Publish-InProgressComment -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -Packages $script:Packages
            $result | Should -BeTrue
            Should -Invoke gh -ModuleName BenchHistoryComment -ParameterFilter {
                ($args -contains 'PATCH') -and ($args -contains 'repos/o/r/issues/comments/42')
            }
            $sent = Get-Content -LiteralPath $script:InProgressCapture -Raw
            $sent | Should -BeLike '*<!-- folo-bench-history-in-progress -->*'
            $sent | Should -BeLike '*events*'
        }
    }

    Context 'when the existing placeholder already matches the current scope' {
        BeforeEach {
            # Render the exact body Publish-InProgressComment will produce for these packages and hand it
            # back as the existing comment, so the idempotency check sees no change. The literal packages
            # here must match $script:Packages passed by the It.
            $canonical = InModuleScope BenchHistoryComment {
                Format-InProgressBody -Marker '<!-- folo-bench-history-pr -->' -Packages 'pool events'
            }
            $list = '[' + (@{ id = 42; body = $canonical; html_url = 'u' } | ConvertTo-Json -Compress) + ']'
            Set-Content -LiteralPath $script:InProgressListFile -Value $list -Encoding utf8 -NoNewline
            Mock gh -ModuleName BenchHistoryComment {
                if ($args -contains 'PATCH') { $global:LASTEXITCODE = 0; return '{"id":42}' }
                $global:LASTEXITCODE = 0
                return (Get-Content -LiteralPath (Join-Path ([System.IO.Path]::GetTempPath()) 'bh-inprogress-existing-list.json') -Raw)
            }
        }

        It 'is a no-op returning false, without patching' {
            $result = Publish-InProgressComment -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -Packages $script:Packages
            $result | Should -BeFalse
            Should -Invoke gh -ModuleName BenchHistoryComment -ParameterFilter { $args -contains 'PATCH' } -Times 0 -Exactly
        }
    }

    Context 'when -WhatIf is passed and no comment exists' {
        BeforeEach {
            Mock gh -ModuleName BenchHistoryComment { $global:LASTEXITCODE = 0; '[]' }
        }

        It 'reports the post without performing it' {
            Publish-InProgressComment -Repo 'o/r' -PrNumber '5' -Marker $script:Marker -Packages $script:Packages -WhatIf | Out-Null
            Should -Invoke gh -ModuleName BenchHistoryComment -ParameterFilter { $args -contains 'POST' } -Times 0 -Exactly
        }
    }

    Context 'input validation' {
        It 'rejects a malformed repository' {
            { Publish-InProgressComment -Repo 'bad repo' -PrNumber '5' -Marker $script:Marker -Packages $script:Packages } |
                Should -Throw '*owner/name*'
        }
    }
}

Describe 'Format-InProgressBody (unexported string transform)' {
    It 'renders a single impacted package with singular wording' {
        InModuleScope BenchHistoryComment {
            $body = Format-InProgressBody -Marker '<!-- folo-bench-history-pr -->' -Packages 'solo'
            $body.Contains('benchmarking the 1 package impacted by this PR (`solo`).') | Should -BeTrue
        }
    }

    It 'sorts and de-duplicates multiple impacted packages with plural wording' {
        InModuleScope BenchHistoryComment {
            $body = Format-InProgressBody -Marker '<!-- folo-bench-history-pr -->' -Packages 'beta alpha beta'
            $body.Contains('benchmarking the 2 packages impacted by this PR (`alpha`, `beta`).') | Should -BeTrue
        }
    }

    It 'leads with the dedup and in-progress markers, then the shared header and status' {
        InModuleScope BenchHistoryComment {
            $body = Format-InProgressBody -Marker '<!-- folo-bench-history-pr -->' -Packages 'solo'
            $lines = $body -split "`n"
            $lines[0] | Should -Be '<!-- folo-bench-history-pr -->'
            $lines[1] | Should -Be $script:InProgressMarker
            $body.Contains('### Performance impact (vs `main`)') | Should -BeTrue
            $body | Should -BeLike '*Benchmarking in progress*'
        }
    }
}

