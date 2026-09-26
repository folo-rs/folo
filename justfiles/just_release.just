# Release validation for developers and CI, plus CI-only publication and binary reconciliation.
# scripts/release/ReleasePlan.psm1 connects the version and compatibility gates to their tools.
# Version planning uses the self-contained increment-versions skill or the cargo-release-plan
# CLI documented in packages/cargo-release-plan/book/src/integration/local-planning.md.
# The `gh-` recipes retain the release workflow's PowerShell publication entry points.

[group('release')]
audit:
    cargo audit

# Canary for local tool checks and CI compatibility validation: proves that
# `cargo semver-checks` can actually run on this machine before either trusts its output. When the
# tool fails to *run* - most commonly because the installed cargo-semver-checks is too old for the
# current toolchain's rustdoc JSON format (e.g. "unsupported rustdoc format v57 ... supported
# formats are v53, v55, v56") - a broken tool must never be read as "no breaking changes". This
# canary runs cargo-semver-checks on one small, fast-to-build package against its own `HEAD` as
# the baseline. `--baseline-rev` fixes only the baseline: the candidate is still built from the
# work tree, so the two sides are identical exactly when the work tree has not touched that
# package - which is the normal case, since it is not a package a release change would edit. A
# non-zero exit therefore means "cargo-semver-checks is broken" unless the canary package itself
# was edited, which the diagnostic calls out. Runnable on its own to sanity-check a fresh
# toolchain/tool combination.
[group('release')]
[script]
verify-semver-checks:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    Import-Module "{{ justfile_directory() }}/scripts/release/ReleasePlan.psm1" -Force
    Invoke-VerifySemverCheck

# Verifies the checked-in Cargo.lock is consistent with the workspace manifests, WITHOUT
# modifying it. `cargo metadata --locked` resolves the dependency graph against the committed
# lockfile and exits non-zero if the lock would need to change - the classic trigger being a
# manifest version bumped without its Cargo.lock entry updated to match. The whole workspace
# shares one lockfile, so a single stale entry breaks every `--locked` build, which means a
# release tagged with an inconsistent lock fails all `build-binaries` jobs at once (they check
# out the tag and build `--locked`). The release `publish` job runs this gate before anything is
# published or tagged, turning that latent, workspace-wide failure into an early, actionable
# abort; run it locally after a manual version edit to catch the problem before pushing. It only
# reads the lockfile - the fix for a stale lock is to regenerate it locally and commit it, never
# to regenerate it inside a workflow.
[group('release')]
[script]
verify-lockfile:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    Write-Verbose "Verifying that the checked-in Cargo.lock is consistent with the workspace manifests" -Verbose

    try {
        # Full-graph resolve (no --no-deps) so --locked actually validates the lockfile; the JSON
        # on stdout is discarded, only the exit status matters. Any error message from cargo (e.g.
        # "cannot update the lock file ... because --locked was passed") surfaces above.
        cargo metadata --locked --format-version 1 1> $null
    }
    catch {
        Write-Host ""
        Write-Host "ERROR: the checked-in Cargo.lock is out of date with the workspace manifests (see the error above)." -ForegroundColor Red
        Write-Host ""
        Write-Host "A manifest version was almost certainly changed (e.g. a manual version bump) without updating" -ForegroundColor Red
        Write-Host "the matching Cargo.lock entry. Because the whole workspace shares one lockfile, this makes every" -ForegroundColor Red
        Write-Host "'--locked' build fail, including all release binary builds." -ForegroundColor Red
        Write-Host ""
        Write-Host "Fix: run 'cargo update --workspace' (or 'cargo build') locally to sync Cargo.lock, commit the" -ForegroundColor Red
        Write-Host "result, and re-push. Never regenerate the lockfile inside a workflow - workflows must build the" -ForegroundColor Red
        Write-Host "checked-in lockfile verbatim." -ForegroundColor Red
        exit 1
    }

    # This standalone workspace consumes released path packages but does not share the root
    # lock. Check it during version planning too, before commits or hosted collection.
    # Ref: docs/release-versioning.md#on-a-pull-request.
    Import-Module "{{ justfile_directory() }}/scripts/bench-history/CallerFixture.psm1" -Force
    Assert-CallerFixtureLock -Workspace "{{ justfile_directory() }}"

# Warns about publishable crates that crates.io has never seen. Trusted Publishing cannot
# perform a crate's first-ever publish (it can only be configured on a crate that already
# exists), so a brand-new crate's first release must be done by hand (cargo publish), after
# which Trusted Publishing is configured and subsequent releases flow through the workflow.
# Run locally as an advisory preflight, so the warning surfaces before the CI publish job.
# Best-effort, never a gate: a transient
# crates.io failure degrades to a warning and continues. See docs/release-automation.md.
[group('release')]
[script]
check-never-published:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true
    Import-Module ./scripts/release/ReleaseAutomation.psm1 -Force

    Write-Verbose "Checking crates.io for never-before-published crates" -Verbose
    Test-NeverPublishedCrate -Verbose

# CI version-readiness wrappers.
#
# `RELEASE_PLAN_BASE` supplies the release baseline when set (CI passes the release branch, or
# `merge_group.base_sha` on a queue run); an empty value falls through to the tool default. When
# `GITHUB_OUTPUT` is set, a report is written first so the `semver_targets` step output reaches
# `semver-checks` even if the check then fails. Released-content comparison stays in the Rust
# tool; this recipe is only its command-line entry point.
#
# Fails when a publishable package has unreleased changes without an increment, a derived version
# group disagrees with itself, an in-scope exact requirement is malformed or stale, legacy
# version-group metadata remains, or a package exposing a public dependency stays compatible
# while that dependency releases a breaking change.
[group('release')]
[script]
validate-versions:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    Import-Module "{{ justfile_directory() }}/scripts/release/ReleasePlan.psm1" -Force
    Invoke-ValidateVersions

# The module owns the valid-empty skip and the repeated `-p` argument construction, so an empty
# `package` value is a successful no-op rather than a workspace-wide comparison.
#
# Runs cargo-semver-checks over the packages in the `package` variable; an empty value checks nothing.
[group('release')]
[script]
semver-checks:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    Import-Module "{{ justfile_directory() }}/scripts/release/ReleasePlan.psm1" -Force
    Invoke-SemverCheck -Package "{{ package }}"

# CI-only publication and binary-reconciliation entry points for .github/workflows/release.yml.
# scripts/release/ReleaseAutomation.psm1 owns crate derivation, config injection, publication
# retries, binary reconciliation and target-to-runner mapping. Its Pester suite exercises those
# decisions through `just test-scripts`; the recipes only import and invoke the owning module.
# See docs/release-automation.md for the release workflow's responsibilities.

# Publishes changed crates to crates.io via release-plz using the registry-only config, with
# bounded retries (3 attempts, 15 minutes apart) to ride out crates.io rate limits and the
# short-lived OIDC token. release-plz is idempotent - it re-checks the registry and skips
# already-published versions - so a retry (or a whole re-run) safely resumes a partially
# published release, and each attempt mints a fresh OIDC token. NOT for local use: it
# performs real publishes. See docs/release-automation.md.
[group('automation')]
[script]
gh-release config='release-plz.toml':
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true
    Import-Module ./scripts/release/ReleaseAutomation.psm1 -Force

    Invoke-ReleasePublish -ConfigPath '{{config}}'

# Reconciles tags for published packages and GitHub releases for binary packages. COMMIT freezes
# the successful registry publication's package/version requests; missing tags use a freshly
# verified equivalent main snapshot. Ref: .github/workflows/design.md#publication-and-recovery.
[group('automation')]
[script]
gh-reconcile-releases COMMIT:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true
    Import-Module ./scripts/release/ReleasePublication.psm1 -Force

    Invoke-ReleaseReconciliation -Source "{{ COMMIT }}" -Repository $env:GH_REPO -Verbose

# Reconciles the desired prebuilt-binary archive and checksum pairs against each
# published binary crate's GitHub release, and emits the matrix of incomplete
# platform batches still to build as the `matrix` / `has_binaries` step outputs
# (also printed). Missing GitHub releases are an error because the workflow creates
# them in the preceding step. This is what makes retries self-healing: a re-run
# rebuilds only whatever is still missing, with no hardcoded crate list. See
# .github/workflows/release.yml and docs/release-automation.md.
[group('automation')]
[script]
gh-plan-release-binaries:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true
    Import-Module ./scripts/release/ReleasePublication.psm1 -Force

    Invoke-ReleaseBinaryPlan -Repository $env:GH_REPO -Verbose

# Runs the frozen matrix batch supplied through RELEASE_BINARY_BATCH, rebuilding only incomplete
# asset pairs. The invocation checkout supplies the controller; Rust owns immutable source
# worktrees at the peeled package-tag commits.
[group('automation')]
[script]
gh-build-release-binaries:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true
    Import-Module ./scripts/release/ReleaseBinaries.psm1 -Force

    $output = Join-Path $env:RUNNER_TEMP "release-binaries-$([guid]::NewGuid().ToString('N'))"
    Invoke-ReleaseBinariesHelper -Operation run -InputJson $env:RELEASE_BINARY_BATCH -Repository $env:GH_REPO -OutputPath $output

# Native ZIP/checksum smoke on the same no-upload path used for local release verification.
# The fixture creates its own pinned Git history and never queries or modifies GitHub releases.
[group('release')]
release-binary-smoke:
    cargo test --locked --package release-binaries --test smoke -- --nocapture
    cargo test --locked --package cargo-release-plan --package crp_publication --package crp_native --tests native_binaries:: -- --nocapture
