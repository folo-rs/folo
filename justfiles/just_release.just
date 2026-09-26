# Folo convenience entry points. Portable planning and publication are cargo-release-plan
# operations; GitHub workflows consume the shared action rather than local publisher recipes.
# Ref: docs/release-versioning.md and docs/release-automation.md.

[group('release')]
audit:
    cargo audit

# Verify both committed workspaces without repairing either lockfile.
# Ref: docs/build-and-tooling.md#standard-commands.
[group('release')]
[script]
verify-lockfile:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    cargo metadata --locked --format-version 1 1> $null
    Import-Module "{{ justfile_directory() }}/scripts/bench-history/CallerFixture.psm1" -Force
    Assert-CallerFixtureLock -Workspace "{{ justfile_directory() }}"

# Local version and publication-input validation has the same Rust owner as shared CI checks.
# RELEASE_PLAN_BASE may supply a pinned baseline; otherwise the application's default applies.
[group('release')]
[script]
validate-versions:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    Import-Module "{{ justfile_directory() }}/scripts/release/ReleasePlan.psm1" -Force
    Invoke-ReleaseValidation

# Real tagged-source builds, native archives and isolated upload adapters; no live publication.
# Ref: packages/cargo-release-plan/docs/implementation.md#test-boundaries.
[group('release')]
release-binary-smoke:
    cargo test --locked --package cargo-release-plan --package crp_impl --tests native_binaries:: -- --nocapture
