[group('quality')]
clippy PROFILE='dev':
    cargo clippy {{ target_package }} --profile {{ PROFILE }} --all-targets --all-features --locked -- -D warnings

[group('quality')]
coverage-measure:
    # Before running the tests, we need to clear old test coverage data because the coverage report
    # simply sums up all the data in the target folder, even if it is from old builds.
    cargo llvm-cov clean --workspace

    # This will run tests and generate test coverage data files, to be analyzed separately.
    cargo llvm-cov nextest {{ target_package }} --all-targets --no-report --all-features --locked

# This tool needs a different way to specify the package.
coverage-package := if package == "" { "" } else { "-p " + package }

[group('quality')]
coverage-report:
    cargo llvm-cov report {{ coverage-package }} --open

[group('quality')]
[no-cd]
format:
    cargo +nightly fmt --verbose --all -- --config-path ./unstable-rustfmt.toml
    cargo sort --workspace
    cargo sort-derives

[group('quality')]
format-check:
    cargo +nightly fmt --verbose --all --check
    cargo sort --workspace --check
    cargo sort-derives --check

[group('quality')]
hack:
    cargo hack check --feature-powerset --locked

[group('quality')]
machete:
    cargo machete --skip-target-dir
    cargo +nightly udeps
    cargo +nightly udeps --all-targets

[group('quality')]
careful FILTER="":
    cargo +nightly careful test {{ target_package }} --all-features {{ FILTER }} --locked

[group('quality')]
[script]
check-external-types:
    # Check external types exposed in public APIs of library crates
    $failures = @()
    $total_count = 0
    $checked_count = 0
    
    # Determine which packages to process
    $packages_to_process = @()
    if ("{{ package }}" -eq "") {
        # Get all workspace members
        $workspace_members = Get-ChildItem -Path "packages" -Directory | Where-Object { Test-Path (Join-Path $_.FullName "Cargo.toml") }
        $packages_to_process = $workspace_members | ForEach-Object { $_.Name }
    } else {
        $packages_to_process = @("{{ package }}")
    }
    
    Write-Host "Checking external types for packages: $($packages_to_process -join ', ')" -ForegroundColor Cyan
    Write-Host ""
    
    foreach ($pkg in $packages_to_process) {
        $lib_path = Join-Path "packages" $pkg "src" "lib.rs"
        
        # Skip packages without lib.rs (binary-only crates)
        if (-not (Test-Path $lib_path)) {
            Write-Host "Skipping $pkg (no lib.rs - binary-only crate)" -ForegroundColor DarkGray
            continue
        }
        
        $total_count++
        Write-Host "Checking $pkg..." -ForegroundColor Cyan
        
        $manifest_path = Join-Path "packages" $pkg "Cargo.toml"
        $pkg_dir = Join-Path "packages" $pkg
        
        # Check if this is a proc-macro crate (skip if it is)
        $cargo_toml_content = Get-Content $manifest_path -Raw
        if ($cargo_toml_content -match '\[lib\][^\[]*proc-macro\s*=\s*true') {
            Write-Host "  Skipping $pkg (proc-macro crate)" -ForegroundColor DarkGray
            continue
        }
        
        # Check if package has doc = false (internal packages with no public API)
        if ($cargo_toml_content -match '\[lib\][^\[]*doc\s*=\s*false') {
            Write-Host "  Skipping $pkg (doc = false - internal package)" -ForegroundColor DarkGray
            continue
        }
        
        # Check if package has publish = false (skip if it does)
        if ($cargo_toml_content -match 'publish\s*=\s*false') {
            Write-Host "  Skipping $pkg (not published)" -ForegroundColor DarkGray
            continue
        }
        
        # Check if package has auto-discovered targets (examples/, tests/, benches/ directories)
        # These confuse cargo-check-external-types when Cargo auto-discovers them
        $has_examples_dir = Test-Path (Join-Path $pkg_dir "examples")
        $has_tests_dir = Test-Path (Join-Path $pkg_dir "tests")
        $has_benches_dir = Test-Path (Join-Path $pkg_dir "benches")
        
        if ($has_examples_dir -or $has_tests_dir -or $has_benches_dir) {
            Write-Host "  Skipping $pkg (has auto-discovered targets: examples/tests/benches dirs)" -ForegroundColor DarkGray
            continue
        }
        
        $output = cargo +{{ CARGO_CHECK_EXTERNAL_TYPES_TOOLCHAIN }} check-external-types --manifest-path $manifest_path 2>&1
        $exit_code = $LASTEXITCODE
        
        if ($exit_code -eq 0) {
            Write-Host "  ✓ $pkg passed" -ForegroundColor Green
            $checked_count++
        } else {
            Write-Host "  ✗ $pkg failed" -ForegroundColor Red
            Write-Host $output -ForegroundColor Yellow
            $failures += $pkg
        }
    }
    
    Write-Host ""
    Write-Host "Summary:" -ForegroundColor White
    Write-Host "  Total library crates: $total_count" -ForegroundColor White
    Write-Host "  Passed: $checked_count" -ForegroundColor Green
    Write-Host "  Failed: $($failures.Count)" -ForegroundColor $(if ($failures.Count -eq 0) { "Green" } else { "Red" })
    
    if ($failures.Count -gt 0) {
        Write-Host ""
        Write-Host "Failed packages:" -ForegroundColor Red
        foreach ($failure in $failures) {
            Write-Host "  - $failure" -ForegroundColor Red
        }
        exit 1
    }

# Separate file because it is a giant script.
import 'just_quality_mutants.just'

# Full validation of primary factors, as you would do in a build pipeline before a release.
# Skips some potentially very lengthy validation, which you can run separately via `validate-extra`.
# We assume this is executed on Windows, and will also perform the full validation on Linux.
[group('quality')]
validate: validate-local validate-linux

# Performs the part of the `validate` recipe that must run on Linux, when commanded from Windows.
[group('quality')]
validate-linux:
    wsl -e bash -l -c "just package={{ package }} validate-local"

# Full validation of primary factors, as you would do in a build pipeline before a release.
# Performs validation on the current platform, whatever that may be.
[group('quality')]
validate-local:
    just package={{ package }} format-check
    just package={{ package }} check dev
    just package={{ package }} clippy dev
    just package={{ package }} test-more
    just package={{ package }} test-docs
    just package={{ package }} docs
    just package={{ package }} miri
    just package={{ package }} machete
    just package={{ package }} check-external-types
    just package={{ package }} check release
    just package={{ package }} clippy release
    just package={{ package }} build release

# Validation of extra factors that take potentially too long to run in regular validation.
[group('quality')]
validate-extra: validate-extra-local validate-extra-linux

# Performs the part of the `validate-extra` recipe that must run on Linux, when commanded from Windows.
[group('quality')]
validate-extra-linux:
    wsl -e bash -l -c "just package={{ package }} validate-extra-local"

# Validation of extra factors that take potentially too long to run in regular validation.
# Performs validation on the current platform, whatever that may be.
[group('quality')]
validate-extra-local:
    just package={{ package }} mutants
    just package={{ package }} run-examples
    just package={{ package }} hack
    just package={{ package }} careful