# Guide to releasing a new version

1. Validate everything via `just validate` on Windows (will automatically invoke Linux validation).
1. Execute `just prepare-release` on `main` branch to increment version numbers and update changelogs.
    * Verify pending changes manually and adjust as necessary.
    * Commit and `git push` if satisfied with the changes.
1. Execute `just release` to upload new packages to `crates.io`.
