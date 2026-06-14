# Git workflow

This chapter covers conventions for working with git, GitHub pull requests, and
the release process from the contributor side.

## Creating GitHub pull requests

When creating PRs with `gh pr create`, do not pass the `--body` flag with an
inline string because PowerShell mangles backticks and special characters.
Instead, write the PR body to a temporary file and use `--body-file path/to/file.md`.

## Addressing pull request review comments

When addressing PR review comments, reply to each comment thread with the
disposition (what you did to address it) and mark the thread as resolved after
pushing the commit that addresses it.

## Version bumps

Do not bump crate versions in feature branches. Version bumps are handled by the
release process.
