[group('release')]
audit:
    cargo audit
    
[group('release')]
[script]
prepare-release: verify-semver-checks && check-never-published
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    Write-Verbose "Bumping versions and changelogs via 'release-plz update'" -Verbose
    release-plz update

# Preflight for `just prepare-release`: proves that `cargo semver-checks` can actually run on
# this machine before `release-plz update` starts trusting it. release-plz runs
# cargo-semver-checks to decide whether a bumped version needs to be a *major* bump, but when the
# tool fails to *run* — most commonly because the installed cargo-semver-checks is too old for the
# current toolchain's rustdoc JSON format (e.g. "unsupported rustdoc format v57 ... supported
# formats are v53, v55, v56") — release-plz silently swallows the error and treats it as "no
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

    # A tiny published crate that builds quickly, keeping this preflight cheap. Its own API is
    # irrelevant here — it is only a vehicle for exercising the rustdoc build+parse path that
    # release-plz relies on.
    $package = "folo_utils"

    Write-Verbose "Verifying that cargo-semver-checks can run (canary package '$package', compared against its own HEAD)" -Verbose

    try {
        cargo semver-checks --baseline-rev HEAD -p $package
    }
    catch {
        Write-Host ""
        Write-Host "ERROR: cargo-semver-checks failed to run on the canary package '$package' (see the error above)." -ForegroundColor Red
        Write-Host ""
        Write-Host "This most likely means the installed cargo-semver-checks is too old for the current" -ForegroundColor Red
        Write-Host "toolchain's rustdoc JSON format (look for an 'unsupported rustdoc format' message above)." -ForegroundColor Red
        Write-Host "release-plz silently treats a cargo-semver-checks *failure* as 'no breaking changes', so" -ForegroundColor Red
        Write-Host "releasing now could ship an undetected breaking change. The release has been aborted before" -ForegroundColor Red
        Write-Host "any versions or changelogs were touched." -ForegroundColor Red
        Write-Host ""
        Write-Host "Fix: update the tool with 'cargo install cargo-semver-checks --locked' (or 'just install-tools')" -ForegroundColor Red
        Write-Host "and re-run 'just prepare-release'." -ForegroundColor Red
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
