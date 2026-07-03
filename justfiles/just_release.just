[group('release')]
audit:
    cargo audit
    
[group('release')]
[script]
prepare-release: && check-never-published
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    Write-Verbose "Bumping versions and changelogs via 'release-plz update'" -Verbose
    release-plz update

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
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true
    Import-Module ./scripts/release/ReleaseAutomation.psm1 -Force

    Write-Verbose "Checking crates.io for never-before-published crates" -Verbose
    Test-NeverPublishedCrate -Verbose