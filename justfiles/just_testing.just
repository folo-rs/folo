[group('testing')]
[script]
bench TARGET="":
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    $target_selector = @()

    if ("{{ TARGET }}" -ne "") {
        $target_selector += "--bench"
        $target_selector += "{{ TARGET }}"
    }

    cargo bench {{ target_package }} --all-features $target_selector --locked

# Run Callgrind-based instruction-count benchmarks. Requires Linux (Valgrind)
# and the `gungraun-runner` binary on PATH; both are installed by
# `just install-tools`. Discovers any bench file whose name ends with `_cg`
# (short for Callgrind) under `packages/<pkg>/benches/`. Callers on Windows
# must invoke this through WSL themselves.
[group('testing')]
[script]
bench-cg TARGET="":
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    if (-not $IsLinux) {
        Write-Error "bench-cg requires Linux (Valgrind). On Windows, invoke through WSL from the repo root: wsl -e bash -l -c 'just bench-cg'"
        exit 1
    }

    if (-not (Get-Command gungraun-runner -ErrorAction SilentlyContinue)) {
        Write-Error "bench-cg requires the 'gungraun-runner' binary on PATH. Run 'just install-tools' to install it."
        exit 1
    }

    $package_filter = "{{ package }}"
    $target_filter = "{{ TARGET }}"

    $packages = if ($package_filter -eq "") {
        Get-ChildItem packages -Directory | ForEach-Object { $_.Name }
    } else {
        # Support space-separated package lists (e.g. from cargo-delta output).
        $package_filter -split " " | Where-Object { $_ -ne "" }
    }

    $any_run = $false
    foreach ($pkg in $packages) {
        $benches_dir = Join-Path "packages" $pkg "benches"
        if (-not (Test-Path $benches_dir)) { continue }

        $bench_files = Get-ChildItem $benches_dir -Filter "*_cg.rs"
        foreach ($bench_file in $bench_files) {
            $bench_name = $bench_file.BaseName
            if ($target_filter -ne "" -and $bench_name -ne $target_filter) { continue }

            Write-Host "Running ${pkg}::${bench_name}" -ForegroundColor Cyan
            cargo bench -p $pkg --bench $bench_name --all-features --locked
            $any_run = $true
        }
    }

    if (-not $any_run) {
        $scope = if ($package_filter -eq "") { "the workspace" } else { "package(s) '$package_filter'" }
        $target_clause = if ($target_filter -eq "") { "" } else { " matching target '$target_filter'" }
        Write-Host "No Callgrind benchmarks found in $scope$target_clause." -ForegroundColor Yellow
    }


[group('testing')]
miri FILTER="":
    cargo +{{ env_var('RUST_NIGHTLY') }} miri nextest run {{ target_package }} --no-fail-fast --all-features {{ FILTER }} --locked --no-tests=pass

[group('testing')]
[script]
miri-harder SHARD="" FILTER="":
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    $totalSeeds = 64
    $shard = "{{ SHARD }}"

    if ($shard -eq "") {
        $seedRange = "..$totalSeeds"
    } else {
        $parts = $shard -split "/"

        if ($parts.Length -ne 2) {
            Write-Error "Invalid SHARD value '$shard'. Expected format 'N/M'."
            exit 1
        }

        $shardIndex = 0
        $shardCount = 0
        if (-not [int]::TryParse($parts[0], [ref]$shardIndex) -or -not [int]::TryParse($parts[1], [ref]$shardCount)) {
            Write-Error "Invalid SHARD value '$shard'. N and M must be integers in 'N/M'."
            exit 1
        }

        if ($shardCount -le 0) {
            Write-Error "Invalid SHARD value '$shard'. M must be a positive integer in 'N/M'."
            exit 1
        }

        if ($shardIndex -lt 1 -or $shardIndex -gt $shardCount) {
            Write-Error "Invalid SHARD value '$shard'. N must satisfy 1 <= N <= M in 'N/M'."
            exit 1
        }

        $seedsPerShard = [math]::Floor($totalSeeds / $shardCount)
        if ($seedsPerShard -lt 1) {
            Write-Error "Invalid SHARD value '$shard'. Too many shards for $totalSeeds seeds."
            exit 1
        }

        $start = ($shardIndex - 1) * $seedsPerShard
        if ($shardIndex -eq $shardCount) {
            # Last shard picks up any remainder from uneven division.
            $end = $totalSeeds
        } else {
            $end = $shardIndex * $seedsPerShard
        }
        $seedRange = "$start..$end"
    }

    $env:MIRIFLAGS = "-Zmiri-many-seeds=$seedRange"
    Write-Host "Running miri-harder with seed range: $seedRange"
    cargo +$env:RUST_NIGHTLY miri test {{ target_package }} --no-fail-fast --all-features --lib {{ FILTER }} --locked -- --quiet

[group('testing')]
[script]
miri-example EXAMPLE:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    if ("{{ package }}" -eq "") {
        cargo +$env:RUST_NIGHTLY miri run --all-features --example {{ EXAMPLE }} --locked
    } else {
        cargo +$env:RUST_NIGHTLY miri run -p {{ package }} --all-features --example {{ EXAMPLE }} --locked
    }

[group('testing')]
test FILTER="":
    cargo +{{ env_var('RUST_MSRV') }} nextest run {{ target_package }} --all-features {{ FILTER }} --locked --no-fail-fast

# Run the cargo-bench-history "stress analyze" harness against LOCAL filesystem
# storage: it seeds a giant synthetic benchmark history (default 1000 benchmarks ×
# 2000 commits over 12 months, ~half of which store a run, × 20 discriminant sets
# spanning every engine and the platforms it runs on, plus a short feature branch
# with dirty snapshots and some blessings) into a temporary store, then times each
# analysis mode (history / branch / tip) over it. The dataset is fabricated; the
# point is to observe how long `analyze` takes at scale, not to measure real code.
#
# Pass any harness flags through, e.g. a smaller scenario for a quick check:
#   just bench-history-stress --commits 100 --benchmarks 100
#   just bench-history-stress --verbose --modes history
# Release build, because both seeding and analysis are CPU-bound and a debug build
# distorts the timings badly.
[group('testing')]
[script]
bench-history-stress *ARGS:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    cargo run --release -p cargo-bench-history-stress --locked -- --storage local {{ ARGS }}

# Run the cargo-bench-history "stress analyze" harness against a REAL Azure storage
# account: it seeds the same giant synthetic history into a fresh blob container
# (created and, unless --keep, deleted automatically), then times each analysis
# mode over it. This exercises the real network round-trips that dominate the cost
# against cloud storage.
#
# Requires `az login` (the harness and azcopy authenticate as your Entra user via
# the Azure CLI) and the `azcopy` binary on PATH for the bulk upload — both are
# installed by `just install-tools`. The account defaults to
# BENCH_HISTORY_TEST_AZURE_ACCOUNT from constants.env (shared with the test-azure job);
# override it by passing a name as the first argument. Any further harness flags
# follow, e.g.:
#   just bench-history-stress-azure                       # default account, full scenario
#   just bench-history-stress-azure mystorageacct --keep  # custom account, keep the container
[group('testing')]
[script]
bench-history-stress-azure ACCOUNT=env_var_or_default('BENCH_HISTORY_TEST_AZURE_ACCOUNT', '') *ARGS:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    $account = "{{ ACCOUNT }}"
    if ([string]::IsNullOrEmpty($account)) {
        Write-Error "No storage account specified. Pass it (just bench-history-stress-azure <account> ...) or set BENCH_HISTORY_TEST_AZURE_ACCOUNT in constants.env. Deploy one with infra/azure-bench-history-test/deploy.ps1."
        exit 1
    }

    if (-not (Get-Command azcopy -ErrorAction SilentlyContinue)) {
        Write-Error "bench-history-stress-azure requires the 'azcopy' binary on PATH for the bulk upload. Run 'just install-tools' to install it."
        exit 1
    }

    Write-Host "Running the Azure stress harness against account '$account' (Microsoft Entra ID; requires az login)."
    cargo run --release -p cargo-bench-history-stress --locked -- --storage azure --account $account {{ ARGS }}

# Run the cargo-bench-history Azure storage tests against a local Azurite blob
# emulator (the self-signed account-SAS auth path), as opposed to `test-azure` which
# targets a REAL account via Microsoft Entra ID. Mirrors the CI `test-azurite` job
# for local verification.
#
# Requires the Azurite emulator (installed by `just install-tools`, or manually via
# `npm install -g azurite`). If an emulator is already listening on 127.0.0.1:10000
# this recipe reuses it; otherwise it starts one (in-memory, no persisted state) and
# stops it again when the tests finish — even on failure. BENCH_HISTORY_REQUIRE_AZURITE
# is set so an unreachable emulator is a hard error rather than a silent skip, mirroring
# the CI job. The `azure azurite` test filter selects the Azure-backend tests by the
# naming convention (the `storage::azure` unit tests and the `*_in_azurite` integration
# tests); the `*_in_real_azure` tests it also matches self-skip without ENABLE_AZURE.
[group('testing')]
[script]
test-azurite:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    $blobHost = "127.0.0.1"
    $blobPort = 10000

    # True when something is already accepting connections on the Azurite blob port.
    function Test-AzuriteReachable {
        $client = [System.Net.Sockets.TcpClient]::new()
        try {
            $client.Connect($blobHost, $blobPort)
            return $true
        } catch {
            return $false
        } finally {
            $client.Dispose()
        }
    }

    # Stop a process and (on Windows) its descendants. The Windows `.cmd` shim spawns
    # node as a child, so killing only the shim would orphan the emulator.
    function Stop-AzuriteProcessTree {
        param([Parameter(Mandatory)][int] $ProcessId)

        if ($IsWindows) {
            Get-CimInstance Win32_Process -Filter "ParentProcessId = $ProcessId" -ErrorAction SilentlyContinue |
                ForEach-Object { Stop-AzuriteProcessTree -ProcessId $_.ProcessId }
        }
        Stop-Process -Id $ProcessId -Force -ErrorAction SilentlyContinue
    }

    $startedByUs = $false
    $launcher = $null
    $emulatorPid = 0
    try {
        if (Test-AzuriteReachable) {
            Write-Host "Azurite already reachable at ${blobHost}:${blobPort}; reusing the running emulator."
        } else {
            $shim = if ($IsWindows) { "azurite-blob.cmd" } else { "azurite-blob" }
            $azurite = Get-Command $shim -ErrorAction SilentlyContinue
            if (-not $azurite) {
                throw "Azurite is not installed. Install it with: npm install -g azurite"
            }

            $azuriteArgs = @(
                "--blobHost", $blobHost,
                "--blobPort", "$blobPort",
                "--inMemoryPersistence",
                "--skipApiVersionCheck",
                "--silent",
                "--loose"
            )

            Write-Host "Starting Azurite blob emulator on ${blobHost}:${blobPort} (in-memory)."
            $startParams = @{
                FilePath     = $azurite.Source
                ArgumentList = $azuriteArgs
                PassThru     = $true
            }
            if ($IsWindows) { $startParams.WindowStyle = "Hidden" }
            $launcher = Start-Process @startParams
            $startedByUs = $true

            $ready = $false
            foreach ($attempt in 1..30) {
                if (Test-AzuriteReachable) { $ready = $true; break }
                Start-Sleep -Seconds 1
            }
            if (-not $ready) {
                throw "Azurite did not become ready on ${blobHost}:${blobPort} within 30 seconds."
            }

            # The launcher may be a shim that spawns the real emulator as a child, so
            # resolve the process actually holding the port to stop it directly later.
            $emulatorPid = $launcher.Id
            if ($IsWindows) {
                $listener = Get-NetTCPConnection -LocalPort $blobPort -State Listen -ErrorAction SilentlyContinue |
                    Select-Object -First 1
                if ($listener) { $emulatorPid = [int] $listener.OwningProcess }
            }
            Write-Host "Azurite is ready (pid $emulatorPid)."
        }

        $env:BENCH_HISTORY_REQUIRE_AZURITE = "1"
        # Pin the endpoint to the emulator this recipe manages so a developer's pre-existing
        # AZURITE_BLOB_ENDPOINT cannot redirect the tests to a different target (the recipe
        # would then start/reuse one emulator while the tests hit another — non-hermetic).
        $env:AZURITE_BLOB_ENDPOINT = "http://${blobHost}:${blobPort}/devstoreaccount1"
        Write-Host "Running Azurite-backed tests against ${blobHost}:${blobPort}."
        cargo +$env:RUST_MSRV nextest run -p cargo-bench-history --locked --no-fail-fast azure azurite
    } finally {
        if ($startedByUs) {
            Write-Host "Stopping the Azurite emulator we started."
            if ($emulatorPid -gt 0) { Stop-AzuriteProcessTree -ProcessId $emulatorPid }
            if ($launcher -and $launcher.Id -ne $emulatorPid) {
                Stop-AzuriteProcessTree -ProcessId $launcher.Id
            }
        }
    }

# Run the cargo-bench-history Azure storage tests against a REAL Azure account
# (the Microsoft Entra ID auth path), the counterpart of `test-azurite` which
# covers the local Azurite emulator path. Mirrors the CI `test-azure` job for
# local verification.
#
# Requires `az login` (the tests authenticate as your Entra user via the Azure
# CLI) and a deployed storage account that grants your user the `Storage Blob
# Data Contributor` role — see infra/azure-bench-history-test/ to deploy one. The
# account defaults to BENCH_HISTORY_TEST_AZURE_ACCOUNT from constants.env (shared with
# CI); override it by passing a name: `just test-azure <account>`.
#
# Setting ENABLE_AZURE is what actually runs the real-Azure tests: without it they
# self-skip, so the account name living in constants.env (and thus in every recipe's
# dotenv environment) does not make a plain `just test` run them. This recipe sets
# ENABLE_AZURE only here, so the real-Azure tests run for real only through it.
[group('testing')]
[script]
test-azure ACCOUNT=env_var_or_default('BENCH_HISTORY_TEST_AZURE_ACCOUNT', ''):
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    $account = "{{ ACCOUNT }}"
    if ([string]::IsNullOrEmpty($account)) {
        Write-Error "No storage account specified. Pass it (just test-azure <account>) or set BENCH_HISTORY_TEST_AZURE_ACCOUNT in constants.env. Deploy one with infra/azure-bench-history-test/deploy.ps1."
        exit 1
    }

    $env:BENCH_HISTORY_TEST_AZURE_ACCOUNT = $account
    $env:ENABLE_AZURE = "1"

    Write-Host "Running real-Azure tests against account '$account' (Microsoft Entra ID; requires az login)."
    cargo +$env:RUST_MSRV nextest run -p cargo-bench-history real_azure --locked --no-fail-fast

# Run the benchmark targets once to verify they do not panic. Their measurements are not
# consumed here; this is purely a "the benches still build and run" smoke check, separated
# from "test" because benchmarks are quite slow.
#
# Benches go through `cargo test --benches` rather than nextest: nextest's per-test
# discover-then-dispatch model is incompatible with the Callgrind (`*_cg`) benches, whose
# `harness = false` main() spawns Valgrind for the entire suite in one shot. `cargo test
# --benches` invokes each bench binary's main() once with `--test`, which Criterion treats
# as "run each bench once" and Gungraun treats as its normal one-shot Valgrind run.
# Upstream tracking: https://github.com/gungraun/gungraun/pull/644.
[group('testing')]
[script]
test-benches FILTER="":
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    cargo +$env:RUST_MSRV test {{ target_package }} --all-features {{ FILTER }} --locked --no-fail-fast --benches

# Runs regular tests as well as doing one iteration of benchmarks (the latter via
# `test-benches`). We separate it from "test" because benchmarks are quite slow, so we tend
# to only run them as part of a full validation pass. Tests go through nextest (parallel
# execution, junit reporting, retries, etc.).
[group('testing')]
[script]
test-more FILTER="":
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    cargo +$env:RUST_MSRV nextest run {{ target_package }} --all-features {{ FILTER }} --locked --no-fail-fast --tests
    cargo +$env:RUST_MSRV test {{ target_package }} --all-features {{ FILTER }} --locked --no-fail-fast --benches

[group('testing')]
[script]
test-docs FILTER="":
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    # Extract package name from target_package variable
    $target_package_var = "{{ target_package }}"
    
    if ($target_package_var -eq " --workspace") {
        # Workspace-wide doctests - run normally
        cargo +$env:RUST_MSRV test --all-features --doc {{ FILTER }} --locked
    } else {
        # One or more packages specified via -p flags. We need to check each package
        # individually because binary-only crates do not support doctests.
        $package_names = "{{ package }}".Split(" ", [StringSplitOptions]::RemoveEmptyEntries)

        foreach ($pkg in $package_names) {
            $lib_path = "packages/$pkg/src/lib.rs"

            if (Test-Path $lib_path) {
                cargo +$env:RUST_MSRV test -p $pkg --all-features --doc {{ FILTER }} --locked
            } else {
                # Binary-only crate - skip doctests because Cargo expects library targets
                # for doctests. Binary crates (with only main.rs) cannot contain doctests,
                # so running `cargo test --doc` on them results in an error rather than
                # gracefully running zero tests.
                Write-Host "Skipping doctests for binary-only package: $pkg"
            }
        }
    }

# Run all stand-alone example binaries to ensure they complete without panicking.
# Each example has a 30-second timeout to handle examples that run indefinitely.
# Supports both .rs files and subdirectories with main.rs files.
[group('testing')]
[script]
run-examples:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    # Discover and run all stand-alone example binaries
    $failures = @()
    $total_count = 0
    $success_count = 0
    $timeout_seconds = 30
    
    # Determine which packages to process
    $packages_to_process = @()
    if ("{{ package }}" -eq "") {
        # Get all workspace members
        $workspace_members = Get-ChildItem -Path "packages" -Directory | Where-Object { Test-Path (Join-Path $_.FullName "Cargo.toml") }
        $packages_to_process = $workspace_members | ForEach-Object { $_.Name }
    } else {
        # Support space-separated package lists (e.g. from cargo-delta output).
        $packages_to_process = "{{ package }}".Split(" ", [StringSplitOptions]::RemoveEmptyEntries)
    }
    
    Write-Host "Running examples for packages: $($packages_to_process -join ', ')"
    Write-Host "Timeout per example: $timeout_seconds seconds"
    Write-Host ""
    
    foreach ($pkg in $packages_to_process) {
        $examples_dir = Join-Path "packages" $pkg "examples"
        
        # Skip packages without examples directory (early continue pattern)
        if (-not (Test-Path $examples_dir)) {
            Write-Host "No examples directory found for package '$pkg'" -ForegroundColor DarkGray
            continue
        }
        
        # Pre-compile all examples for this package so compilation time does not eat into the
        # per-example execution timeout. On slow CI runners, first-time compilation can exceed the
        # timeout even though the example itself exits instantly.
        Write-Host "Building examples for package '$pkg'..." -ForegroundColor DarkGray
        $env:IS_TESTING = "1"
        cargo build --package $pkg --examples --all-features --locked
        if ($LASTEXITCODE -ne 0) {
            $failures += "$pkg::* (build failed)"
            continue
        }

        # Find .rs files directly in examples directory
        $example_files = Get-ChildItem -Path $examples_dir -Filter "*.rs" | Where-Object { $_.Name -ne "mod.rs" }
        
        # Find subdirectories with main.rs files
        $example_subdirs = Get-ChildItem -Path $examples_dir -Directory | Where-Object { 
            Test-Path (Join-Path $_.FullName "main.rs") 
        }
        
        # Examples that are expected to panic or fail and should be excluded from run-examples
        $excluded_examples = @(
            "nm_otel_console"     # This example runs an infinite loop by design
            "nm_otel_readme"      # This example runs an infinite loop by design
            "alloc_tracker_panic_on_alloc_demo"  # This example intentionally panics to demonstrate functionality
        )
        
        # Process .rs files
        foreach ($example_file in $example_files) {
            $example_name = $example_file.BaseName
            
            # Skip excluded examples
            if ($excluded_examples -contains $example_name) {
                Write-Host "Skipping excluded example '$example_name' in package '$pkg'" -ForegroundColor DarkGray
                continue
            }
            
            $total_count++
            
            Write-Host "Running example '$example_name' in package '$pkg'..." -ForegroundColor Cyan
            
            try {
                # Run the example with a timeout to prevent hanging
                $job = Start-Job -ScriptBlock {
                    param($pkg, $example_name)
                    $env:IS_TESTING = "1"
                    & cargo run --package $pkg --example $example_name --all-features --locked 2>&1
                    return $LASTEXITCODE
                } -ArgumentList $pkg, $example_name
                
                $completed = Wait-Job -Job $job -Timeout $timeout_seconds
                
                if ($completed) {
                    $result = Receive-Job -Job $job
                    $exit_code = $result[-1]  # Last item should be the exit code
                    $output = $result[0..($result.Length-2)] -join "`n"  # All output except exit code
                    
                    if ($exit_code -eq 0) {
                        Write-Host "✓ Example '$example_name' in package '$pkg' completed successfully" -ForegroundColor Green
                        $success_count++
                    } else {
                        Write-Host "✗ Example '$example_name' in package '$pkg' failed with exit code $exit_code" -ForegroundColor Red
                        if ($output.Trim() -ne "") {
                            Write-Host "Output:" -ForegroundColor Yellow
                            Write-Host $output -ForegroundColor DarkYellow
                        }
                        $failures += "$pkg::$example_name (exit code $exit_code)"
                    }
                } else {
                    Write-Host "✗ Example '$example_name' in package '$pkg' timed out after $timeout_seconds seconds" -ForegroundColor Red
                    $failures += "$pkg::$example_name (timeout)"
                    Stop-Job -Job $job
                }
                
                Remove-Job -Job $job -Force
                
            } catch {
                Write-Host "✗ Example '$example_name' in package '$pkg' failed with exception: $($_.Exception.Message)" -ForegroundColor Red
                $failures += "$pkg::$example_name (exception: $($_.Exception.Message))"
            }
        }
        
        # Process subdirectory examples
        foreach ($example_subdir in $example_subdirs) {
            $example_name = $example_subdir.Name
            
            # Skip excluded examples
            if ($excluded_examples -contains $example_name) {
                Write-Host "Skipping excluded example '$example_name' in package '$pkg'" -ForegroundColor DarkGray
                continue
            }
            
            $total_count++
            
            Write-Host "Running example '$example_name' in package '$pkg'..." -ForegroundColor Cyan
            
            try {
                # Run the example with a timeout to prevent hanging
                $job = Start-Job -ScriptBlock {
                    param($pkg, $example_name)
                    $env:IS_TESTING = "1"
                    & cargo run --package $pkg --example $example_name --all-features --locked 2>&1
                    return $LASTEXITCODE
                } -ArgumentList $pkg, $example_name
                
                $completed = Wait-Job -Job $job -Timeout $timeout_seconds
                
                if ($completed) {
                    $result = Receive-Job -Job $job
                    $exit_code = $result[-1]  # Last item should be the exit code
                    $output = $result[0..($result.Length-2)] -join "`n"  # All output except exit code
                    
                    if ($exit_code -eq 0) {
                        Write-Host "✓ Example '$example_name' in package '$pkg' completed successfully" -ForegroundColor Green
                        $success_count++
                    } else {
                        Write-Host "✗ Example '$example_name' in package '$pkg' failed with exit code $exit_code" -ForegroundColor Red
                        if ($output.Trim() -ne "") {
                            Write-Host "Output:" -ForegroundColor Yellow
                            Write-Host $output -ForegroundColor DarkYellow
                        }
                        $failures += "$pkg::$example_name (exit code $exit_code)"
                    }
                } else {
                    Write-Host "✗ Example '$example_name' in package '$pkg' timed out after $timeout_seconds seconds" -ForegroundColor Red
                    $failures += "$pkg::$example_name (timeout)"
                    Stop-Job -Job $job
                }
                
                Remove-Job -Job $job -Force
                
            } catch {
                Write-Host "✗ Example '$example_name' in package '$pkg' failed with exception: $($_.Exception.Message)" -ForegroundColor Red
                $failures += "$pkg::$example_name (exception: $($_.Exception.Message))"
            }
        }
    }
    
    Write-Host ""
    Write-Host "Summary:" -ForegroundColor White
    Write-Host "  Total examples: $total_count" -ForegroundColor White
    Write-Host "  Successful: $success_count" -ForegroundColor Green
    Write-Host "  Failed: $($failures.Count)" -ForegroundColor $(if ($failures.Count -eq 0) { "Green" } else { "Red" })
    
    if ($failures.Count -gt 0) {
        Write-Host ""
        Write-Host "Failed examples:" -ForegroundColor Red
        foreach ($failure in $failures) {
            Write-Host "  - $failure" -ForegroundColor Red
        }
        exit 1
    }
