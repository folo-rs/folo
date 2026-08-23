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
            Mock Start-Sleep -ModuleName Retry { }
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

Describe 'Format-StalenessWarning (unexported string transform)' {
    Context 'given a related, positive distance' {
        It 'words the plural and singular forms' {
            InModuleScope BenchHistoryComment {
                (Format-StalenessWarning -Distance @{ Related = $true; Behind = 3 }) | Should -BeLike '*3 commits behind HEAD*'
                (Format-StalenessWarning -Distance @{ Related = $true; Behind = 1 }) | Should -BeLike '*1 commit behind HEAD*'
                (Format-StalenessWarning -Distance @{ Related = $true; Behind = 1 }) | Should -Not -BeLike '*1 commits*'
            }
        }
    }

    Context 'given an unrelated, zero, or null distance' {
        It 'falls back to the numberless "out of date" wording' {
            InModuleScope BenchHistoryComment {
                # Unrelated histories, a non-positive distance, and a null (failed-lookup) distance all
                # collapse to the same numberless wording - the branch both staleness paths rely on.
                (Format-StalenessWarning -Distance @{ Related = $false; Behind = 0 }) | Should -BeLike '*out of date*'
                (Format-StalenessWarning -Distance @{ Related = $true;  Behind = 0 }) | Should -BeLike '*out of date*'
                (Format-StalenessWarning -Distance $null) | Should -BeLike '*out of date*'
                (Format-StalenessWarning -Distance $null) | Should -Not -BeLike '*behind HEAD*'
            }
        }
    }
}

Describe 'Add-StalenessBannerIfBehind (mocked gh api)' {
    BeforeAll {
        $script:SelfMarker       = '<!-- folo-bench-history-pr -->'
        $script:SelfCommitPrefix = '<!-- folo-bench-history-commit:'
        $script:SelfAnalyzed     = '1111111111111111111111111111111111111111'
    }

    BeforeEach {
        # A freshly composed results body - the dedup marker, the hidden analyzed-commit marker, then
        # findings - rewritten IN PLACE by the function, so each test starts from a clean copy on disk.
        $script:SelfBodyFile = Join-Path ([System.IO.Path]::GetTempPath()) ("bh-selfcheck-$([guid]::NewGuid().ToString('n')).md")
        $composed = @(
            '<!-- folo-bench-history-pr -->'
            '<!-- folo-bench-history-commit:1111111111111111111111111111111111111111 -->'
            ''
            '### Performance impact (vs `main`)'
            ''
            '✅ No benchmark regressions detected against `main`.'
        ) -join "`n"
        Set-Content -LiteralPath $script:SelfBodyFile -Value $composed -Encoding utf8 -NoNewline
    }

    AfterEach {
        Remove-Item -LiteralPath $script:SelfBodyFile -ErrorAction SilentlyContinue
    }

    Context 'when the PR has advanced past the analyzed commit' {
        BeforeEach {
            # The live PR head has moved on (pulls .head.sha) and is three commits ahead of the analyzed
            # commit (compare ahead_by), so the composed results are already stale.
            Mock gh -ModuleName BenchHistoryComment {
                foreach ($a in $args) { if ($a -like 'repos/*/pulls/*') { $global:LASTEXITCODE = 0; return '2222222222222222222222222222222222222222' } }
                foreach ($a in $args) { if ($a -like 'repos/*/compare/*') { $global:LASTEXITCODE = 0; return '{"status":"ahead","ahead_by":3}' } }
                $global:LASTEXITCODE = 0
                return ''
            }
        }

        It 'injects an N-commits-behind banner and preserves the analyzed-commit marker and findings' {
            $result = Add-StalenessBannerIfBehind -Repo 'o/r' -PrNumber '5' -Marker $script:SelfMarker -AnalyzedSha $script:SelfAnalyzed -BodyFile $script:SelfBodyFile
            $result | Should -BeTrue
            $body = Get-Content -LiteralPath $script:SelfBodyFile -Raw
            $body | Should -BeLike '*3 commits behind HEAD*'
            $body | Should -BeLike '*[!WARNING]*'
            # The hidden analyzed-commit marker must survive so a later run can still parse it.
            $body | Should -BeLike "*$($script:SelfCommitPrefix)$($script:SelfAnalyzed)*"
            # The findings themselves are preserved, not clobbered by the banner.
            $body | Should -BeLike '*No benchmark regressions detected*'
        }
    }

    Context 'when the PR advanced by exactly one commit' {
        BeforeEach {
            Mock gh -ModuleName BenchHistoryComment {
                foreach ($a in $args) { if ($a -like 'repos/*/pulls/*') { $global:LASTEXITCODE = 0; return '2222222222222222222222222222222222222222' } }
                foreach ($a in $args) { if ($a -like 'repos/*/compare/*') { $global:LASTEXITCODE = 0; return '{"status":"ahead","ahead_by":1}' } }
                $global:LASTEXITCODE = 0
                return ''
            }
        }

        It 'uses the singular "commit"' {
            Add-StalenessBannerIfBehind -Repo 'o/r' -PrNumber '5' -Marker $script:SelfMarker -AnalyzedSha $script:SelfAnalyzed -BodyFile $script:SelfBodyFile | Out-Null
            $body = Get-Content -LiteralPath $script:SelfBodyFile -Raw
            $body | Should -BeLike '*1 commit behind HEAD*'
            $body | Should -Not -BeLike '*1 commits behind*'
        }
    }

    Context 'when the PR head shares no history with the analyzed commit (force-push)' {
        BeforeEach {
            Mock gh -ModuleName BenchHistoryComment {
                foreach ($a in $args) { if ($a -like 'repos/*/pulls/*') { $global:LASTEXITCODE = 0; return '2222222222222222222222222222222222222222' } }
                foreach ($a in $args) { if ($a -like 'repos/*/compare/*') { $global:LASTEXITCODE = 1; return 'gh: No common ancestor for the two commits (HTTP 404)' } }
                $global:LASTEXITCODE = 0
                return ''
            }
        }

        It 'injects a numberless "out of date" banner' {
            $result = Add-StalenessBannerIfBehind -Repo 'o/r' -PrNumber '5' -Marker $script:SelfMarker -AnalyzedSha $script:SelfAnalyzed -BodyFile $script:SelfBodyFile
            $result | Should -BeTrue
            $body = Get-Content -LiteralPath $script:SelfBodyFile -Raw
            $body | Should -BeLike '*out of date*'
            $body | Should -Not -BeLike '*behind HEAD*'
        }
    }

    Context 'when the compare lookup fails for a transient reason' {
        BeforeEach {
            Mock gh -ModuleName BenchHistoryComment {
                foreach ($a in $args) { if ($a -like 'repos/*/pulls/*') { $global:LASTEXITCODE = 0; return '2222222222222222222222222222222222222222' } }
                # A non-404 compare failure is a genuine error Get-CommitsBehind rethrows; the self-check
                # is best-effort and must degrade to the numberless wording rather than fail the post.
                foreach ($a in $args) { if ($a -like 'repos/*/compare/*') { $global:LASTEXITCODE = 1; return 'HTTP 500: Internal Server Error' } }
                $global:LASTEXITCODE = 0
                return ''
            }
        }

        It 'degrades to the numberless "out of date" banner instead of throwing' {
            $result = Add-StalenessBannerIfBehind -Repo 'o/r' -PrNumber '5' -Marker $script:SelfMarker -AnalyzedSha $script:SelfAnalyzed -BodyFile $script:SelfBodyFile
            $result | Should -BeTrue
            $body = Get-Content -LiteralPath $script:SelfBodyFile -Raw
            $body | Should -BeLike '*out of date*'
        }
    }

    Context 'when the PR still points at the analyzed commit' {
        BeforeEach {
            # pulls .head.sha returns the SAME sha the run analyzed: the results are current, so the body
            # must be posted untouched and no compare call is made.
            Mock gh -ModuleName BenchHistoryComment {
                foreach ($a in $args) { if ($a -like 'repos/*/pulls/*') { $global:LASTEXITCODE = 0; return '1111111111111111111111111111111111111111' } }
                $global:LASTEXITCODE = 0
                return ''
            }
        }

        It 'leaves the body unchanged, returns false, and never calls the compare api' {
            $before = Get-Content -LiteralPath $script:SelfBodyFile -Raw
            $result = Add-StalenessBannerIfBehind -Repo 'o/r' -PrNumber '5' -Marker $script:SelfMarker -AnalyzedSha $script:SelfAnalyzed -BodyFile $script:SelfBodyFile
            $result | Should -BeFalse
            (Get-Content -LiteralPath $script:SelfBodyFile -Raw) | Should -BeExactly $before
            Should -Invoke gh -ModuleName BenchHistoryComment -ParameterFilter { [bool]($args | Where-Object { $_ -like 'repos/*/compare/*' }) } -Times 0 -Exactly
        }
    }

    Context 'when the live PR head cannot be read' {
        BeforeEach {
            # The pulls lookup fails (a transient gh/API hiccup): best-effort means post as composed.
            Mock gh -ModuleName BenchHistoryComment {
                foreach ($a in $args) { if ($a -like 'repos/*/pulls/*') { $global:LASTEXITCODE = 1; return 'HTTP 503: Service Unavailable' } }
                $global:LASTEXITCODE = 0
                return ''
            }
        }

        It 'leaves the body unchanged and returns false without throwing' {
            $before = Get-Content -LiteralPath $script:SelfBodyFile -Raw
            $result = Add-StalenessBannerIfBehind -Repo 'o/r' -PrNumber '5' -Marker $script:SelfMarker -AnalyzedSha $script:SelfAnalyzed -BodyFile $script:SelfBodyFile
            $result | Should -BeFalse
            (Get-Content -LiteralPath $script:SelfBodyFile -Raw) | Should -BeExactly $before
        }
    }

    Context 'when the live head comes back malformed' {
        BeforeEach {
            Mock gh -ModuleName BenchHistoryComment {
                foreach ($a in $args) { if ($a -like 'repos/*/pulls/*') { $global:LASTEXITCODE = 0; return 'not-a-sha' } }
                $global:LASTEXITCODE = 0
                return ''
            }
        }

        It 'posts as composed (false) rather than guessing at staleness' {
            $before = Get-Content -LiteralPath $script:SelfBodyFile -Raw
            $result = Add-StalenessBannerIfBehind -Repo 'o/r' -PrNumber '5' -Marker $script:SelfMarker -AnalyzedSha $script:SelfAnalyzed -BodyFile $script:SelfBodyFile
            $result | Should -BeFalse
            (Get-Content -LiteralPath $script:SelfBodyFile -Raw) | Should -BeExactly $before
        }
    }

    Context 'input validation' {
        It 'rejects a non-hex analyzed SHA' {
            { Add-StalenessBannerIfBehind -Repo 'o/r' -PrNumber '5' -Marker $script:SelfMarker -AnalyzedSha 'nope' -BodyFile $script:SelfBodyFile } |
                Should -Throw '*40-character hex*'
        }

        It 'throws when the body file is missing' {
            $missing = Join-Path ([System.IO.Path]::GetTempPath()) ("bh-missing-$([guid]::NewGuid().ToString('n')).md")
            { Add-StalenessBannerIfBehind -Repo 'o/r' -PrNumber '5' -Marker $script:SelfMarker -AnalyzedSha $script:SelfAnalyzed -BodyFile $missing } |
                Should -Throw '*does not exist*'
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

Describe 'Format-CollectionScope (unexported string transform)' {
    It 'renders a single impacted package with singular wording' {
        InModuleScope BenchHistoryComment {
            $scope = Format-CollectionScope -Packages 'solo' -Verb 'benchmarked'
            $scope | Should -Be '**Collection scope:** benchmarked the 1 package impacted by this PR (`solo`).'
        }
    }

    It 'sorts and de-duplicates multiple impacted packages with plural wording' {
        InModuleScope BenchHistoryComment {
            $scope = Format-CollectionScope -Packages 'beta alpha  beta' -Verb 'benchmarking'
            $scope | Should -Be '**Collection scope:** benchmarking the 2 packages impacted by this PR (`alpha`, `beta`).'
        }
    }
}

Describe 'Get-SeriesCoverage (unexported report projection)' {
    BeforeAll {
        # Every distinct coverage situation the analysis can report, as the report.json the analyze job
        # leaves behind. Shared by the verdict and body suites below so one situation is described once
        # and checked on every surface it reaches.
        function Format-CoverageReportJson {
            param(
                [Parameter(Mandatory)][AllowNull()][hashtable] $Census,
                [bool] $Notable = $false
            )

            $report = if ($null -eq $Census) { @{ notable = $Notable } } else { @{ notable = $Notable; census = $Census } }
            return ($report | ConvertTo-Json -Depth 6)
        }

        function Write-CoverageScratch {
            param(
                [Parameter(Mandatory)][string] $ReportJson,
                [string] $Notable = 'false',
                [string] $Summary = '## Findings'
            )

            $dir = Join-Path ([System.IO.Path]::GetTempPath()) ("bh-scratch-$([guid]::NewGuid().ToString('n'))")
            New-Item -ItemType Directory -Path $dir | Out-Null
            Set-Content -LiteralPath (Join-Path $dir 'report.json') -Value $ReportJson -Encoding utf8
            Set-Content -LiteralPath (Join-Path $dir 'notable.txt') -Value $Notable -Encoding utf8
            Set-Content -LiteralPath (Join-Path $dir 'summary.md') -Value $Summary -Encoding utf8
            return $dir
        }

        $script:ScratchRoots = @()
    }

    AfterAll {
        foreach ($root in $script:ScratchRoots) {
            Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
        }
    }

    It 'transports the state and counts the report states (<Name>)' -TestCases @(
        @{
            Name = 'nothing in scope'
            Census = @{ total = 4; in_scope = 0; judged = 0; unjudged = 4; coverage = 'nothing_in_scope'
                reasons = @(@{ reason = 'ghost'; count = 4 })
            }
            State = 'nothing_in_scope'; Judged = 0; InScope = 0; Shortfalls = 0
        }
        @{
            Name = 'nothing judged'
            Census = @{ total = 3; in_scope = 3; judged = 0; unjudged = 3; coverage = 'nothing_judged'
                reasons = @(@{ reason = 'too_few_base_commits'; count = 3 })
            }
            State = 'nothing_judged'; Judged = 0; InScope = 3; Shortfalls = 1
        }
        @{
            Name = 'partial'
            Census = @{ total = 5; in_scope = 4; judged = 3; unjudged = 2; coverage = 'partial'
                reasons = @(@{ reason = 'ghost'; count = 1 }, @{ reason = 'too_few_points'; count = 1 })
            }
            State = 'partial'; Judged = 3; InScope = 4; Shortfalls = 1
        }
        @{
            Name = 'full'
            Census = @{ total = 2; in_scope = 2; judged = 2; unjudged = 0; coverage = 'full'; reasons = @() }
            State = 'full'; Judged = 2; InScope = 2; Shortfalls = 0
        }
    ) {
        $dir = Write-CoverageScratch -ReportJson (Format-CoverageReportJson -Census $Census)
        $script:ScratchRoots += $dir
        $expected = @{ State = $State; Judged = $Judged; InScope = $InScope; Shortfalls = $Shortfalls }
        InModuleScope BenchHistoryComment -Parameters @{ Dir = $dir; Expected = $expected } {
            param($Dir, $Expected)

            $coverage = Get-SeriesCoverage -ReportPath (Join-Path $Dir 'report.json')
            $coverage.State | Should -Be $Expected.State
            $coverage.Judged | Should -Be $Expected.Judged
            $coverage.InScope | Should -Be $Expected.InScope
            # Ghosts are out of scope by construction, so they never reach the shortfall list the
            # comment explains; the "Collection scope" line covers them instead.
            $reasons = @(@($coverage.Shortfalls) | ForEach-Object { [string]$_.Reason })
            $reasons.Count | Should -Be $Expected.Shortfalls
            $reasons | Should -Not -Contain 'ghost'
        }
    }

    It 'reports no series when the report carries no census (total collect failure)' {
        $dir = Write-CoverageScratch -ReportJson (Format-CoverageReportJson -Census $null)
        $script:ScratchRoots += $dir
        InModuleScope BenchHistoryComment -Parameters @{ Dir = $dir } {
            param($Dir)

            $coverage = Get-SeriesCoverage -ReportPath (Join-Path $Dir 'report.json')
            $coverage.State | Should -Be 'no_series'
            $coverage.InScope | Should -Be 0
        }
    }

    It 'reports no series when the report file is absent' {
        InModuleScope BenchHistoryComment {
            $missing = Join-Path ([System.IO.Path]::GetTempPath()) "bh-absent-$([guid]::NewGuid().ToString('n')).json"
            (Get-SeriesCoverage -ReportPath $missing).State | Should -Be 'no_series'
        }
    }

    It 'refuses to guess a state the report does not carry' {
        # A census without a coverage field comes from a tool that predates the projection. Re-deriving
        # the verdict here is exactly the drift the projection exists to prevent, so it stays unknown.
        $census = @{ total = 3; in_scope = 3; judged = 1; unjudged = 2
            reasons = @(@{ reason = 'too_few_points'; count = 2 })
        }
        $dir = Write-CoverageScratch -ReportJson (Format-CoverageReportJson -Census $census)
        $script:ScratchRoots += $dir
        InModuleScope BenchHistoryComment -Parameters @{ Dir = $dir } {
            param($Dir)

            (Get-SeriesCoverage -ReportPath (Join-Path $Dir 'report.json')).State | Should -Be 'unknown'
        }
    }

    It 'does not throw or invent a shortfall when the census omits fields' {
        # A truncated document, or one from a tool version older than a field being read, must degrade
        # to zeroes and an empty breakdown - `Set-StrictMode -Version Latest` turns a bare property read
        # on a missing field into a terminating error that would fail the whole comment step.
        $dir = Write-CoverageScratch -ReportJson '{"notable":false,"census":{"coverage":"partial"}}'
        $script:ScratchRoots += $dir
        InModuleScope BenchHistoryComment -Parameters @{ Dir = $dir } {
            param($Dir)

            $coverage = Get-SeriesCoverage -ReportPath (Join-Path $Dir 'report.json')
            $coverage.State | Should -Be 'partial'
            $coverage.Judged | Should -Be 0
            $coverage.InScope | Should -Be 0
            $coverage.Total | Should -Be 0
            @($coverage.Shortfalls).Count | Should -Be 0
            # An entry naming no reason cannot be described to a reader, so it is dropped rather than
            # rendered as an empty clause.
            Format-CoverageShortfall -Shortfalls @($coverage.Shortfalls) | Should -Be 'no reason was reported'
        }
    }
}

Describe 'Format-CoverageVerdict (unexported string transform)' {
    It 'claims an all-clear only where every in-scope series was judged (<Name>)' -TestCases @(
        @{ Name = 'full'; State = 'full'; AllClear = $true }
        @{ Name = 'partial'; State = 'partial'; AllClear = $false }
        @{ Name = 'nothing judged'; State = 'nothing_judged'; AllClear = $false }
        @{ Name = 'nothing in scope'; State = 'nothing_in_scope'; AllClear = $false }
        @{ Name = 'no series'; State = 'no_series'; AllClear = $false }
        @{ Name = 'a state added after this module'; State = 'invented_later'; AllClear = $false }
        @{ Name = 'a report that stated no state'; State = 'unknown'; AllClear = $false }
    ) {
        InModuleScope BenchHistoryComment -Parameters @{ State = $State; AllClear = $AllClear } {
            param($State, $AllClear)

            $coverage = @{
                State      = $State
                Judged     = 2
                InScope    = 4
                Total      = 5
                Shortfalls = @(@{ Reason = 'too_few_points'; Count = 2 })
            }
            $verdict = (Format-CoverageVerdict -Coverage $coverage) -join "`n"
            # The checkmark is the at-a-glance signal a reader scans for, so it must appear for exactly
            # one state and the warning sign for every other.
            $verdict.Contains([char]0x2705) | Should -Be $AllClear
            $verdict.Contains([char]0x26A0) | Should -Be (-not $AllClear)
        }
    }

    It 'explains each in-scope shortfall in the reader''s terms (<Reason>)' -TestCases @(
        @{ Reason = 'too_few_points'; Phrase = '2 series with too few measurements in the analyzed window' }
        @{ Reason = 'too_few_points_since_blessing'; Phrase = '2 series with too few measurements since being blessed' }
        @{ Reason = 'not_measured_on_branch'; Phrase = '2 series not measured on this branch' }
        @{ Reason = 'too_few_base_commits'; Phrase = '2 series with too little `main` history to compare against' }
    ) {
        InModuleScope BenchHistoryComment -Parameters @{ Reason = $Reason; Phrase = $Phrase } {
            param($Reason, $Phrase)

            $coverage = @{
                State      = 'partial'
                Judged     = 2
                InScope    = 4
                Total      = 4
                Shortfalls = @(@{ Reason = $Reason; Count = 2 })
            }
            ((Format-CoverageVerdict -Coverage $coverage) -join "`n").Contains($Phrase) | Should -BeTrue
        }
    }

    It 'names a reason it does not recognize instead of mislabelling it' {
        # The failure this guards against: a reason added to the tool later being described as short
        # base-branch history, which would send the reader chasing the wrong explanation.
        InModuleScope BenchHistoryComment {
            $coverage = @{
                State      = 'partial'
                Judged     = 2
                InScope    = 4
                Total      = 4
                Shortfalls = @(@{ Reason = 'invented_later'; Count = 2 })
            }
            $verdict = (Format-CoverageVerdict -Coverage $coverage) -join "`n"
            $verdict.Contains('2 series not judged for an unrecognized reason (`invented_later`)') | Should -BeTrue
            $verdict.Contains('`main` history') | Should -BeFalse
        }
    }

    It 'lists every reason when several fell short' {
        InModuleScope BenchHistoryComment {
            $coverage = @{
                State      = 'nothing_judged'
                Judged     = 0
                InScope    = 5
                Total      = 7
                Shortfalls = @(
                    @{ Reason = 'too_few_points'; Count = 3 }
                    @{ Reason = 'not_measured_on_branch'; Count = 2 }
                )
            }
            $verdict = (Format-CoverageVerdict -Coverage $coverage) -join "`n"
            $verdict.Contains('Not assessed: 3 series with too few measurements in the analyzed window; 2 series not measured on this branch.') | Should -BeTrue
        }
    }
}

Describe 'Format-PrBenchCommentBody (composed rolling comment)' {
    BeforeAll {
        $script:AnalyzedSha = 'a' * 40
        $script:CommitPrefix = '<!-- folo-bench-history-analyzed: '
        $script:BodyScratchRoots = @()

        function Write-BodyScratch {
            param(
                [Parameter(Mandatory)][AllowNull()][hashtable] $Census,
                [string] $Notable = 'false',
                [string] $Summary = '## Findings',
                [bool] $NotableFlag = $false
            )

            $report = if ($null -eq $Census) { @{ notable = $NotableFlag } } else { @{ notable = $NotableFlag; census = $Census } }
            $dir = Join-Path ([System.IO.Path]::GetTempPath()) ("bh-body-$([guid]::NewGuid().ToString('n'))")
            New-Item -ItemType Directory -Path $dir | Out-Null
            Set-Content -LiteralPath (Join-Path $dir 'report.json') -Value ($report | ConvertTo-Json -Depth 6) -Encoding utf8
            Set-Content -LiteralPath (Join-Path $dir 'notable.txt') -Value $Notable -Encoding utf8
            Set-Content -LiteralPath (Join-Path $dir 'summary.md') -Value $Summary -Encoding utf8
            $script:BodyScratchRoots += $dir
            return $dir
        }

        function Get-ComposedBody {
            param(
                [Parameter(Mandatory)][string] $ScratchDir,
                [string] $ArtifactUrl = ''
            )

            return Format-PrBenchCommentBody `
                -Marker $script:Marker `
                -CommitMarkerPrefix $script:CommitPrefix `
                -AnalyzedSha $script:AnalyzedSha `
                -Packages 'alpha beta' `
                -ScratchDir $ScratchDir `
                -ReportArtifactUrl $ArtifactUrl
        }
    }

    AfterAll {
        foreach ($root in $script:BodyScratchRoots) {
            Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
        }
    }

    It 'renders the verdict the report warrants (<Name>)' -TestCases @(
        @{
            Name = 'absent census'
            Census = $null
            Expected = @('**No benchmark results were produced**')
            Forbidden = @('No benchmark regressions detected')
        }
        @{
            Name = 'every series a ghost'
            Census = @{ total = 4; in_scope = 0; judged = 0; unjudged = 4; coverage = 'nothing_in_scope'
                reasons = @(@{ reason = 'ghost'; count = 4 })
            }
            Expected = @('**Nothing in scope was measured**')
            Forbidden = @('No benchmark regressions detected')
        }
        @{
            Name = 'in scope but nothing judged'
            Census = @{ total = 3; in_scope = 3; judged = 0; unjudged = 3; coverage = 'nothing_judged'
                reasons = @(@{ reason = 'too_few_base_commits'; count = 3 })
            }
            Expected = @(
                '**Nothing was judged.** None of the 3 in-scope series could be assessed'
                'Not assessed: 3 series with too little `main` history to compare against.'
            )
            Forbidden = @('No benchmark regressions detected')
        }
        @{
            Name = 'partial: too few points'
            Census = @{ total = 4; in_scope = 4; judged = 3; unjudged = 1; coverage = 'partial'
                reasons = @(@{ reason = 'too_few_points'; count = 1 })
            }
            Expected = @(
                '**No regressions among the 3 of 4 series that could be judged.**'
                'The remaining 1 were not assessed: 1 series with too few measurements in the analyzed window.'
            )
            Forbidden = @('No benchmark regressions detected')
        }
        @{
            Name = 'partial: too few points since blessing'
            Census = @{ total = 4; in_scope = 4; judged = 3; unjudged = 1; coverage = 'partial'
                reasons = @(@{ reason = 'too_few_points_since_blessing'; count = 1 })
            }
            Expected = @('1 series with too few measurements since being blessed')
            Forbidden = @('No benchmark regressions detected')
        }
        @{
            Name = 'partial: not measured on branch'
            Census = @{ total = 4; in_scope = 4; judged = 3; unjudged = 1; coverage = 'partial'
                reasons = @(@{ reason = 'not_measured_on_branch'; count = 1 })
            }
            # The bug this replaces: a series the branch never measured was reported as ordinary
            # base-history warm-up, which is a different situation with a different remedy.
            Expected = @('1 series not measured on this branch')
            Forbidden = @('`main` history to compare against')
        }
        @{
            Name = 'partial: too few base commits'
            Census = @{ total = 4; in_scope = 4; judged = 3; unjudged = 1; coverage = 'partial'
                reasons = @(@{ reason = 'too_few_base_commits'; count = 1 })
            }
            Expected = @('1 series with too little `main` history to compare against')
            Forbidden = @('No benchmark regressions detected')
        }
        @{
            Name = 'mixed reasons'
            Census = @{ total = 9; in_scope = 7; judged = 4; unjudged = 5; coverage = 'partial'
                reasons = @(
                    @{ reason = 'ghost'; count = 2 }
                    @{ reason = 'too_few_points'; count = 2 }
                    @{ reason = 'not_measured_on_branch'; count = 1 }
                )
            }
            Expected = @(
                'The remaining 3 were not assessed: 2 series with too few measurements in the analyzed window; 1 series not measured on this branch.'
            )
            Forbidden = @('ghost')
        }
        @{
            Name = 'a reason added after this module was written'
            Census = @{ total = 4; in_scope = 4; judged = 3; unjudged = 1; coverage = 'partial'
                reasons = @(@{ reason = 'invented_later'; count = 1 })
            }
            Expected = @('1 series not judged for an unrecognized reason (`invented_later`)')
            Forbidden = @('`main` history to compare against')
        }
        @{
            Name = 'full coverage'
            Census = @{ total = 3; in_scope = 3; judged = 3; unjudged = 0; coverage = 'full'; reasons = @() }
            Expected = @('No benchmark regressions detected against `main` (all 3 in-scope series judged).')
            Forbidden = @('not assessed')
        }
        @{
            Name = 'a multi-metric ghost, counted per metric series'
            Census = @{ total = 3; in_scope = 1; judged = 1; unjudged = 2; coverage = 'full'
                reasons = @(@{ reason = 'ghost'; count = 2 })
            }
            Expected = @('(all 1 in-scope series judged)')
            Forbidden = @('3 in-scope')
        }
    ) {
        $body = Get-ComposedBody -ScratchDir (Write-BodyScratch -Census $Census)
        foreach ($phrase in $Expected) {
            $body.Contains($phrase) | Should -BeTrue -Because "the comment states '$phrase':`n$body"
        }
        foreach ($phrase in $Forbidden) {
            $body.Contains($phrase) | Should -BeFalse -Because "the comment must not state '$phrase':`n$body"
        }
    }

    It 'leads every body with the dedup marker, the analyzed-commit marker and the shared header' {
        $body = Get-ComposedBody -ScratchDir (Write-BodyScratch -Census $null)
        $lines = $body -split "`n"
        $lines[0] | Should -Be $script:Marker
        $lines[1] | Should -Be "$script:CommitPrefix$script:AnalyzedSha -->"
        $body.Contains('### Performance impact (vs `main`)') | Should -BeTrue
        $body.Contains("**Analyzed commit:** $script:AnalyzedSha") | Should -BeTrue
        $body.Contains('**Collection scope:** benchmarked the 2 packages impacted by this PR (`alpha`, `beta`).') | Should -BeTrue
        $body.Contains('This check is advisory and never blocks the merge.') | Should -BeTrue
    }

    It 'carries the condensed summary, the artifact link and the footer when there are findings' {
        $dir = Write-BodyScratch -Census @{ total = 2; in_scope = 2; judged = 2; unjudged = 0; coverage = 'full'; reasons = @() } `
            -Notable 'true' -Summary '## Regression in `alpha`' -NotableFlag $true
        $body = Get-ComposedBody -ScratchDir $dir -ArtifactUrl 'https://example.invalid/artifact'
        $body.Contains('## Regression in `alpha`') | Should -BeTrue
        $body.Contains('[Download the full report bundle](https://example.invalid/artifact)') | Should -BeTrue
        $body.Contains('**How to read this**') | Should -BeTrue
        # The coverage verdict belongs to the silent path only: with findings on display, the reader is
        # already looking at what moved.
        $body.Contains('in-scope series judged') | Should -BeFalse
    }

    It 'rejects an analyzed SHA that is not a full commit id' {
        { Get-ComposedBody -ScratchDir (Write-BodyScratch -Census $null) } | Should -Not -Throw
        {
            Format-PrBenchCommentBody -Marker $script:Marker -CommitMarkerPrefix $script:CommitPrefix `
                -AnalyzedSha 'abc' -Packages 'alpha' -ScratchDir (Write-BodyScratch -Census $null)
        } | Should -Throw '*40-character hex*'
    }

    It 'throws when the analysis artefacts are missing' {
        $empty = Join-Path ([System.IO.Path]::GetTempPath()) ("bh-body-empty-$([guid]::NewGuid().ToString('n'))")
        New-Item -ItemType Directory -Path $empty | Out-Null
        $script:BodyScratchRoots += $empty
        { Get-ComposedBody -ScratchDir $empty } | Should -Throw '*notable flag*'
        { Get-ComposedBody -ScratchDir (Join-Path $empty 'nowhere') } | Should -Throw '*scratch directory*'
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

