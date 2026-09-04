[group('release')]
audit:
    cargo audit
    
[group('release')]
[script]
prepare-release: verify-semver-checks && check-never-published
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    Import-Module "{{ justfile_directory() }}/scripts/release/ReleasePlan.psm1" -Force
    Write-Verbose "Bumping versions and changelogs via 'release-plz update'" -Verbose
    Invoke-ReleasePlzUpdate

# Preflight for `just prepare-release`: proves that `cargo semver-checks` can actually run on
# this machine before `release-plz update` starts trusting it. release-plz runs
# cargo-semver-checks to decide whether a bumped version needs to be a *major* bump, but when the
# tool fails to *run* - most commonly because the installed cargo-semver-checks is too old for the
# current toolchain's rustdoc JSON format (e.g. "unsupported rustdoc format v57 ... supported
# formats are v53, v55, v56") - release-plz silently swallows the error and treats it as "no
# breaking changes". That turns a broken tool into an undetected breaking release. This canary
# runs cargo-semver-checks on one small, fast-to-build package, comparing it against its own `HEAD`
# so the two sides are byte-identical and the *only* way the check can fail is the tool itself
# failing to run. A non-zero exit therefore means "cargo-semver-checks is broken", not "a real
# breaking change was found", so we abort the release before anything is changed. Runnable on its
# own to sanity-check a fresh toolchain/tool combination.
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

# Warns about publishable crates that crates.io has never seen. Trusted Publishing cannot
# perform a crate's first-ever publish (it can only be configured on a crate that already
# exists), so a brand-new crate's first release must be done by hand (cargo publish), after
# which Trusted Publishing is configured and subsequent releases flow through the workflow.
# Run as a post-step of `prepare-release`, so the warning surfaces while a release is being
# prepared instead of failing the CI publish job later. Best-effort, never a gate: a transient
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

# Fails when any publishable package has unreleased changes past its version-anchor without
# an increment, or any version group declares inconsistent versions. Thin wrapper over
# `Invoke-ValidateVersions` (`cargo release-plan check --format github`). `RELEASE_PLAN_BASE`
# supplies the release baseline when set (CI passes the pull-request base SHA or
# `merge_group.base_sha`); an empty value falls through to the tool default. When
# `GITHUB_OUTPUT` is set, a report is written first so the `released` step output reaches
# `semver-checks` even if the check then fails. Released-content comparison stays in the Rust tool.
[group('release')]
[script]
validate-versions:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    Import-Module "{{ justfile_directory() }}/scripts/release/ReleasePlan.psm1" -Force
    Invoke-ValidateVersions

# Writes release-plan evidence under DIR and runs cargo-semver-checks for the same explicit
# consumer-contract targets selected for CI. The module owns validation, target selection, and
# exit handling; this recipe is only its command-line entry point.
[group('release')]
[script]
release-report DIR:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    Import-Module "{{ justfile_directory() }}/scripts/release/ReleasePlan.psm1" -Force
    Invoke-ReleaseReport -OutDir "{{ DIR }}"

# Runs cargo-semver-checks for the package list supplied by CI. The module owns the valid-empty
# skip and repeated `-p` argument construction.
[group('release')]
[script]
semver-checks:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    Import-Module "{{ justfile_directory() }}/scripts/release/ReleasePlan.psm1" -Force
    Invoke-SemverCheck -Package "{{ package }}"

# Prints the dependency-first analysis batches from a release-plan report as a JSON array.
# Mutually dependent packages share a batch and are analyzed to a fixed point. The
# increment-versions skill redirects this to a working file and documents the field names.
[group('release')]
[script]
release-analysis-order REPORT:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    Import-Module "{{ justfile_directory() }}/scripts/release/ReleasePlan.psm1" -Force
    Get-ReleasePlanAnalysisBatch -ReportPath "{{ REPORT }}" | ConvertTo-Json -Depth 3 -AsArray

# Fails unless every package reached by the approved semantic changes is already published.
[group('release')]
[script]
check-increment-published REPORT DECISIONS:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    Import-Module "{{ justfile_directory() }}/scripts/release/ReleasePlan.psm1" -Force
    Assert-IncrementPackagePublished -ReportPath "{{ REPORT }}" -DecisionPath "{{ DECISIONS }}"

# Converts approved semantic change levels into cargo-release-plan's mechanical plan format.
[group('release')]
[script]
create-release-plan REPORT DECISIONS PLAN:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    Import-Module "{{ justfile_directory() }}/scripts/release/ReleasePlan.psm1" -Force
    New-ReleasePlanFile -ReportPath "{{ REPORT }}" -DecisionPath "{{ DECISIONS }}" -PlanPath "{{ PLAN }}"

# Applies an approved increment plan: package versions, version-group expansion, `=`-pin
# rewrites, and a workspace lockfile refresh. The plan file is an uncommitted working-tree
# artifact; the version check verifies manifest state, not intent.
[group('release')]
[script]
apply-release-plan PLAN:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    Import-Module "{{ justfile_directory() }}/scripts/release/ReleasePlan.psm1" -Force
    Invoke-ApplyReleasePlan -PlanPath "{{ PLAN }}"
