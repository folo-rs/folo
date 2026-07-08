# Runs cargo-delta to determine which packages are impacted by changes on the current branch
# compared to origin/main. Outputs a space-separated list of affected package names.
[group('delta')]
[script]
delta:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    # The analyze-current / analyze-baseline-via-worktree / run / parse pipeline lives in the
    # Pester-tested Delta module, shared with the CI `delta` job so the two cannot drift apart.
    # See scripts/build/Delta.psm1.
    Import-Module "{{ justfile_directory() }}/scripts/build/Delta.psm1" -Force

    $affected = @(Invoke-CargoDelta)

    Write-Host ""
    if ($affected.Count -gt 0) {
        Write-Host "Affected packages ($($affected.Count)):"
        foreach ($pkg in $affected) {
            Write-Host "  - $pkg"
        }
        Write-Host ""
        Write-Host "DELTA_PACKAGES=$($affected -join ' ')"
    } else {
        Write-Host "No packages affected by changes."
        Write-Host "DELTA_PACKAGES="
    }

# Runs delta analysis and then validates only the affected packages.
[group('delta')]
[script]
delta-validate:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    Import-Module "{{ justfile_directory() }}/scripts/build/Delta.psm1" -Force

    $packages = @(Invoke-CargoDelta)
    if ($packages.Count -eq 0) {
        Write-Host "No packages affected. Nothing to validate."
        exit 0
    }

    $packageList = $packages -join " "
    Write-Host "Running validate-local for: $packageList"
    just package="$packageList" validate-local

# Runs delta analysis and then performs extended validation on the affected packages.
[group('delta')]
[script]
delta-validate-extra:
    Set-StrictMode -Version Latest
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    Import-Module "{{ justfile_directory() }}/scripts/build/Delta.psm1" -Force

    $packages = @(Invoke-CargoDelta)
    if ($packages.Count -eq 0) {
        Write-Host "No packages affected. Nothing to validate."
        exit 0
    }

    $packageList = $packages -join " "
    Write-Host "Running validate-extra-local for: $packageList"
    just package="$packageList" validate-extra-local
