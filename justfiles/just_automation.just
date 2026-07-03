# Recipes intended ONLY for automation (GitHub Actions), not local use. Each is
# prefixed `gh-` to signal that. They assume the workspace's committed
# `.cargo/bench_history.toml` already points at the prod Azure history store
# (account folohistory) and that the runner is signed in to Azure. We may factor
# these out into dedicated GitHub Actions later (see folo-rs/folo#284), so keep them
# thin wrappers over the tool, free of local-developer conveniences.

# Collects the whole workspace's benchmark history (excluding the slow, special-purpose
# `benchmarks` package) into the prod Azure store, keyed by the current commit. The
# machine partition for noisy wall-clock engines is auto-detected; deterministic engines
# (Callgrind, allocation/time counters) are hardware-independent. `--skip-existing` makes a
# re-run on an unchanged commit a no-op append (each already-stored object is skipped, not
# overwritten), so a re-triggered run of an already-collected commit never bumps the
# cache-invalidation marker the `analyze` job's read-through cache depends on — a re-run of
# the same commit is not "better" data, so there is nothing to replace.
[group('automation')]
gh-collect-bench-history:
    cargo run -p cargo-bench-history --bin cargo-bench-history --locked -- collect --workspace --exclude benchmarks --skip-existing --verbose

# Analyzes the collected history for regressions across ALL engines, target triples and
# machine keys, writing two working-directory artefacts: bench-history-report.md (the
# Markdown report) and bench-history-notable.txt (`true`/`false`, the JSON `notable`
# flag). Findings never fail the recipe (the tool always exits 0); the workflow files a
# rolling issue when notable. `auto` mode on a clean `main` checkout resolves to
# long-range history-trend detection. A single analyze pass emits both the Markdown and
# JSON reports via the per-format output toggles (`--no-text --markdown --json`).
#
# `--cache` mirrors fetched cloud objects under `bench-history-cache/` (relative to the
# repo root, the working directory here) so the bulk history is downloaded at most once
# across runs instead of in full every run. The bench-history.yml `analyze` job persists
# that directory between runs via `actions/cache`; its `path:` MUST match the directory
# named here. The cloud history is append-only (collection uses
# `--skip-existing`), so the cache only ever tops up — a stale mirror is wiped via the
# cloud-side marker only after a deliberate `--overwrite`/`prune`.
[group('automation')]
[script]
gh-analyze-bench-history:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    $reportPath = Join-Path (Get-Location) "bench-history-report.md"
    $jsonPath = Join-Path (Get-Location) "bench-history-report.json"
    $notablePath = Join-Path (Get-Location) "bench-history-notable.txt"
    $common = @('--engine', 'all', '--target-triple', 'all', '--machine-key', 'all')

    Write-Verbose "Analyzing benchmark history in a single pass; writing Markdown to $reportPath and JSON to $jsonPath, mirroring fetched cloud objects under bench-history-cache/ so the history is downloaded at most once across runs." -Verbose
    cargo run -p cargo-bench-history --bin cargo-bench-history --locked -- analyze @common --cache=bench-history-cache --no-text --markdown $reportPath --json $jsonPath
    Write-Verbose "Computing the notable flag from the JSON report." -Verbose
    $json = Get-Content -Path $jsonPath -Raw | ConvertFrom-Json
    $notable = if ($json.notable) { 'true' } else { 'false' }
    Set-Content -Path $notablePath -Value $notable -Encoding utf8
    Write-Verbose "Analysis complete: notable=$notable (report at $reportPath)." -Verbose

# The single triple -> runner mapping for prebuilt release binaries, shared by the
# reconciliation recipe below and .github/workflows/release.yml. Native runners, one per
# target, no cross-compilation. GitHub offers `-latest` only for x64 Linux/Windows and
# macOS (macos-latest is arm64); ARM Linux/Windows have no `-latest` alias, so they are
# pinned by version. Bump the pinned images when newer ones ship. Intel macOS is not built.
_release-targets := '''
    x86_64-unknown-linux-gnu ubuntu-latest
    aarch64-unknown-linux-gnu ubuntu-24.04-arm
    x86_64-pc-windows-msvc windows-latest
    aarch64-pc-windows-msvc windows-11-arm
    aarch64-apple-darwin macos-latest
'''

# Writes a CI-only copy of release-plz.toml to {{output}} with `git_release_enable = true`
# set for each publishable binary crate (derived from cargo metadata), leaving everything
# else untouched. The committed config keeps releases OFF at the workspace level so the many
# library/`_impl` crates never spawn GitHub releases; enabling it only for the derived binary
# crates means a newly-added binary tool is covered with no config edit and no library crate
# is ever released. The caller writes this OUTSIDE the working tree so `cargo publish` never
# sees a dirty repo. See .github/workflows/release.yml and docs/release-automation.md.
[group('automation')]
[script]
gh-compose-release-config output:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    Write-Verbose "Deriving publishable binary crates from cargo metadata." -Verbose
    $metadata = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    $binCrates = $metadata.packages |
        Where-Object { ($null -eq $_.publish) -or ($_.publish.Count -gt 0) } |
        Where-Object { $_.targets | Where-Object { $_.kind -contains 'bin' } } |
        Select-Object -ExpandProperty name |
        Sort-Object -Unique
    Write-Verbose "Binary crates: $($binCrates -join ', ')" -Verbose

    $lines = [System.Collections.Generic.List[string]]::new()
    foreach ($line in Get-Content -Path 'release-plz.toml') { $lines.Add($line) }

    foreach ($crate in $binCrates) {
        # The `name = "<crate>"` key is unique and is the first key of its [[package]]
        # block, so inserting right after it places the flag inside that block. An exact
        # match avoids `cargo-bench-history` colliding with `cargo-bench-history-core`.
        $needle = 'name = "' + $crate + '"'
        $nameIdx = -1
        for ($i = 0; $i -lt $lines.Count; $i++) {
            if ($lines[$i].Trim() -eq $needle) { $nameIdx = $i; break }
        }
        if ($nameIdx -ge 0) {
            $already = $false
            for ($j = $nameIdx + 1; $j -lt $lines.Count; $j++) {
                $trimmed = $lines[$j].Trim()
                if ($trimmed.StartsWith('[')) { break }
                if ($trimmed -match '^git_release_enable\s*=') { $already = $true; break }
            }
            if (-not $already) {
                Write-Verbose "Enabling git releases for existing entry '$crate'." -Verbose
                $lines.Insert($nameIdx + 1, 'git_release_enable = true')
            }
        } else {
            Write-Verbose "Adding a [[package]] entry enabling git releases for '$crate'." -Verbose
            $lines.Add('')
            $lines.Add('[[package]]')
            $lines.Add($needle)
            $lines.Add('git_release_enable = true')
        }
    }

    Set-Content -Path '{{output}}' -Value $lines -Encoding utf8
    Write-Verbose "Wrote CI release-plz config to {{output}}." -Verbose

# Publishes changed crates to crates.io via release-plz using the composed CI config, with
# bounded retries (3 attempts, 15 minutes apart) to ride out crates.io rate limits and the
# short-lived OIDC token. release-plz is idempotent — it re-checks the registry and skips
# already-published versions — so a retry (or a whole re-run) safely resumes a partially
# published release, and each attempt mints a fresh OIDC token. NOT for local use: it
# performs real publishes. See docs/release-automation.md.
[group('automation')]
[script]
gh-release config:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    $attempts = 3
    $delaySeconds = 900  # 15 minutes
    for ($n = 1; $n -le $attempts; $n++) {
        Write-Verbose "release-plz release attempt $n of $attempts (config {{config}})." -Verbose
        # release-plz exiting non-zero is an expected, retryable outcome here, so suspend
        # the native-error toggle for the call and inspect the exit code ourselves.
        $PSNativeCommandUseErrorActionPreference = $false
        release-plz release --config '{{config}}'
        $exitCode = $LASTEXITCODE
        $PSNativeCommandUseErrorActionPreference = $true

        if ($exitCode -eq 0) {
            Write-Verbose "release-plz release succeeded on attempt $n." -Verbose
            return
        }
        if ($n -eq $attempts) {
            throw "release-plz release failed after $attempts attempts (last exit code $exitCode)."
        }
        Write-Warning "Attempt $n failed (exit $exitCode); retrying in $delaySeconds seconds..."
        Start-Sleep -Seconds $delaySeconds
    }

# Reconciles the desired prebuilt-binary archives against what is already uploaded to each
# published binary crate's GitHub release, and emits the matrix of missing (crate, target)
# pairs still to build as the `matrix` / `has_binaries` step outputs (also printed). This is
# what makes retries self-healing: a re-run rebuilds only whatever is still missing, with no
# hardcoded crate list. Safe to run locally — with no releases yet it reports an empty
# matrix. See .github/workflows/release.yml and docs/release-automation.md.
[group('automation')]
[script]
gh-plan-release-binaries:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    $targets = foreach ($row in ('{{_release-targets}}' -split "`n")) {
        $parts = $row.Trim() -split '\s+'
        if ($parts.Count -eq 2) {
            [pscustomobject]@{ triple = $parts[0]; os = $parts[1] }
        }
    }

    Write-Verbose "Deriving publishable binary crates and versions from cargo metadata." -Verbose
    $metadata = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    $crates = $metadata.packages |
        Where-Object { ($null -eq $_.publish) -or ($_.publish.Count -gt 0) } |
        Where-Object { $_.targets | Where-Object { $_.kind -contains 'bin' } }

    $include = [System.Collections.Generic.List[object]]::new()
    foreach ($crate in $crates) {
        $name = $crate.name
        $version = $crate.version
        $tag = "$name-v$version"

        # A missing release throws (native-error toggle is on); treat that as "skip, nothing
        # to attach binaries to yet". A release that exists but has no assets is distinct and
        # correctly yields every target as missing.
        $releaseExists = $true
        $viewed = $null
        try { $viewed = gh release view $tag --json assets 2>$null } catch { $releaseExists = $false }
        if (-not $releaseExists) {
            Write-Verbose "No release '$tag' yet; skipping." -Verbose
            continue
        }
        $assets = @(($viewed | ConvertFrom-Json).assets.name)

        foreach ($target in $targets) {
            $archive = "$name-v$version-$($target.triple).zip"
            if ($assets -contains $archive) { continue }
            Write-Verbose "Missing archive: $archive" -Verbose
            $include.Add([pscustomobject]@{
                name = $name; version = $version; tag = $tag; triple = $target.triple; os = $target.os
            })
        }
    }

    # matrix.include must always be a JSON array (ConvertTo-Json unwraps a single element).
    $matrix = if ($include.Count -eq 0) {
        '[]'
    } else {
        $json = ConvertTo-Json -InputObject @($include) -Compress -Depth 5
        if ($json.TrimStart().StartsWith('[')) { $json } else { "[$json]" }
    }
    $hasBinaries = if ($include.Count -gt 0) { 'true' } else { 'false' }

    Write-Host "Missing (crate, target) archives: $($include.Count)"
    Write-Host "matrix=$matrix"
    Write-Host "has_binaries=$hasBinaries"
    if ($env:GITHUB_OUTPUT) {
        "matrix=$matrix" | Add-Content -Path $env:GITHUB_OUTPUT
        "has_binaries=$hasBinaries" | Add-Content -Path $env:GITHUB_OUTPUT
    }
