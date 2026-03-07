# Runs cargo-delta to determine which packages are impacted by changes on the current branch
# compared to origin/main. Outputs a space-separated list of affected package names.
[group('delta')]
[script]
delta:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    # We need the full history of both branches to compare them. If we are in a shallow clone
    # (e.g. in CI), we need to unshallow first.
    $is_shallow = git rev-parse --is-shallow-repository
    if ($is_shallow -eq "true") {
        git fetch --unshallow origin main
    } else {
        git fetch origin main
    }

    # Use a temporary directory for analysis artifacts.
    $temp_dir = Join-Path ([System.IO.Path]::GetTempPath()) "cargo-delta-$(Get-Random)"
    New-Item -ItemType Directory -Path $temp_dir -Force | Out-Null

    try {
        $baseline_json = Join-Path $temp_dir "baseline.json"
        $current_json = Join-Path $temp_dir "current.json"
        $delta_json = Join-Path $temp_dir "delta.json"
        $config_path = (Resolve-Path "delta.toml").Path

        # Analyze the current branch first (we are already on it).
        Write-Host "Analyzing current branch..."
        cargo delta -c $config_path analyze | Out-File -FilePath $current_json -Encoding utf8

        # Use a git worktree to analyze origin/main without switching branches.
        $worktree_dir = Join-Path $temp_dir "main-worktree"
        git worktree add $worktree_dir origin/main --quiet

        try {
            Write-Host "Analyzing baseline (origin/main)..."
            Push-Location $worktree_dir
            cargo delta -c $config_path analyze | Out-File -FilePath $baseline_json -Encoding utf8
            Pop-Location
        } finally {
            git worktree remove $worktree_dir --force
        }

        # Compare to find impacted crates.
        Write-Host "Computing delta..."
        cargo delta -c $config_path run --baseline $baseline_json --current $current_json | Out-File -FilePath $delta_json -Encoding utf8

        # Parse the delta output to extract the "Affected" package list.
        $delta = Get-Content $delta_json -Raw | ConvertFrom-Json
        $affected = $delta.Affected

        if ($affected -and $affected.Count -gt 0) {
            $package_list = $affected -join " "
            Write-Host ""
            Write-Host "Affected packages ($($affected.Count)):"
            foreach ($pkg in $affected) {
                Write-Host "  - $pkg"
            }
            Write-Host ""
            Write-Host "DELTA_PACKAGES=$package_list"
        } else {
            Write-Host ""
            Write-Host "No packages affected by changes."
            Write-Host "DELTA_PACKAGES="
        }
    } finally {
        Remove-Item -Path $temp_dir -Recurse -Force -ErrorAction SilentlyContinue
    }

# Runs delta analysis and then validates only the affected packages.
[group('delta')]
[script]
delta-validate:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    # Capture delta output and extract the package list.
    $output = just delta 2>&1 | Out-String
    Write-Host $output

    $match = [regex]::Match($output, "DELTA_PACKAGES=(.*)")
    if (-not $match.Success) {
        Write-Error "Failed to parse delta output."
        exit 1
    }

    $packages = $match.Groups[1].Value.Trim()

    if ($packages -eq "") {
        Write-Host "No packages affected. Nothing to validate."
        exit 0
    }

    Write-Host "Running validate-local for: $packages"
    just package="$packages" validate-local

# Runs delta analysis and then performs extended validation on the affected packages.
[group('delta')]
[script]
delta-validate-extra:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    # Capture delta output and extract the package list.
    $output = just delta 2>&1 | Out-String
    Write-Host $output

    $match = [regex]::Match($output, "DELTA_PACKAGES=(.*)")
    if (-not $match.Success) {
        Write-Error "Failed to parse delta output."
        exit 1
    }

    $packages = $match.Groups[1].Value.Trim()

    if ($packages -eq "") {
        Write-Host "No packages affected. Nothing to validate."
        exit 0
    }

    Write-Host "Running validate-extra-local for: $packages"
    just package="$packages" validate-extra-local
