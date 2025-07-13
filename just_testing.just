[group('testing')]
[script]
bench TARGET="":
    $target_selector = @()

    if ("{{ TARGET }}" -ne "") {
        $target_selector += "--bench"
        $target_selector += "{{ TARGET }}"
    }

    cargo bench {{ target_package }} --all-features $target_selector --locked

[group('testing')]
miri FILTER="":
    cargo +nightly miri nextest run {{ target_package }} --no-fail-fast --all-features {{ FILTER }} --locked

[group('testing')]
[script]
miri-example EXAMPLE:
    if ("{{ package }}" -eq "") {
        cargo +nightly miri run --all-features --example {{ EXAMPLE }} --locked
    } else {
        cargo +nightly miri run -p {{ package }} --all-features --example {{ EXAMPLE }} --locked
    }

[group('testing')]
test FILTER="":
    cargo nextest run {{ target_package }} --all-features {{ FILTER }} --locked

# We run benches separately because they are slow in Nextest multi-process mode,
# probably due to the Gnuplot integration that spawns an external process.
[group('testing')]
test-benches FILTER="":
    cargo test --benches --all-features {{ FILTER }} --locked

[group('testing')]
test-docs FILTER="":
    cargo test {{ target_package }} --all-features --doc {{ FILTER }} --locked

# Run all stand-alone example binaries to ensure they complete without panicking.
# Each example has a 30-second timeout to handle examples that run indefinitely.
# Supports both .rs files and subdirectories with main.rs files.
[group('testing')]
[script]
run-examples:
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
        $packages_to_process = @("{{ package }}")
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
        
        # Find .rs files directly in examples directory
        $example_files = Get-ChildItem -Path $examples_dir -Filter "*.rs" | Where-Object { $_.Name -ne "mod.rs" }
        
        # Find subdirectories with main.rs files
        $example_subdirs = Get-ChildItem -Path $examples_dir -Directory | Where-Object { 
            Test-Path (Join-Path $_.FullName "main.rs") 
        }
        
        # Process .rs files
        foreach ($example_file in $example_files) {
            $example_name = $example_file.BaseName
            $total_count++
            
            Write-Host "Running example '$example_name' in package '$pkg'..." -ForegroundColor Cyan
            
            try {
                # Run the example with a timeout to prevent hanging
                $job = Start-Job -ScriptBlock {
                    param($pkg, $example_name)
                    $env:IS_TESTING = "1"
                    & cargo run --package $pkg --example $example_name --locked 2>&1
                    return $LASTEXITCODE
                } -ArgumentList $pkg, $example_name
                
                $completed = Wait-Job -Job $job -Timeout $timeout_seconds
                
                if ($completed) {
                    $result = Receive-Job -Job $job
                    $exit_code = $result[-1]  # Last item should be the exit code
                    
                    if ($exit_code -eq 0) {
                        Write-Host "✓ Example '$example_name' in package '$pkg' completed successfully" -ForegroundColor Green
                        $success_count++
                    } else {
                        Write-Host "✗ Example '$example_name' in package '$pkg' failed with exit code $exit_code" -ForegroundColor Red
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
                    
                    if ($exit_code -eq 0) {
                        Write-Host "✓ Example '$example_name' in package '$pkg' completed successfully" -ForegroundColor Green
                        $success_count++
                    } else {
                        Write-Host "✗ Example '$example_name' in package '$pkg' failed with exit code $exit_code" -ForegroundColor Red
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
