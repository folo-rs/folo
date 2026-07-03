# Installs the complete set of Rust toolchains and other tools for local PC development.
[group('setup')]
[script]
install-tools:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    # This "full install" logic is specific to local PC setup. The GitHub workflows reuse the same
    # version numbers but have their own logic for what tooling needs to be installed in which jobs.

    # First, we install/upgrade the stable toolchain in `rust-toolchain.toml`.
    rustup toolchain install

    # Then we install the other toolchains used by specific commands.
    rustup toolchain install $env:RUST_MSRV
    rustup toolchain install $env:RUST_NIGHTLY --component miri --component rustfmt --component rust-src --component llvm-tools-preview

    cargo install cargo-machete cargo-nextest release-plz cargo-semver-checks cargo-audit cargo-hack cargo-llvm-cov cargo-sort cargo-sort-derives cargo-udeps cargo-careful cargo-ensure-no-default-features cargo-delta cargo-freeze-deps --locked

    # cargo-mutants transitively depends on `winapi 0.2.8`, which only defines its
    # types for `target_arch = "x86"` / `"x86_64"` and fails to compile on aarch64
    # with errors like `cannot find type 'ULONG_PTR' in this scope`. Until that
    # transitive dependency is updated upstream, skip the install on ARM. The CI
    # `mutants` job only runs on x86_64 runners, so this only affects local dev on
    # ARM machines (where running mutation testing locally is not supported).
    $arch = [System.Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture
    if ($arch -ne [System.Runtime.InteropServices.Architecture]::Arm64) {
        cargo install cargo-mutants --locked
    } else {
        Write-Host "Skipping cargo-mutants install on ARM64 (winapi 0.2.8 has no aarch64 support)."
    }

    # gungraun-runner drives Valgrind for Callgrind-based instruction-count benchmarks. Only
    # useful on Linux (Valgrind is Linux-only); on Windows and macOS the Callgrind bench binaries
    # are no-op stubs.
    #
    # The Callgrind bench binary is just a shim that forwards its CLI args (including the libtest
    # --list / --format / --ignored flags that `cargo bench` and `nextest` use during target
    # enumeration) to gungraun-runner via an encoded payload. Without gungraun-runner on $PATH,
    # any invocation of a `*_cg` bench binary fails with "Failed to run benchmarks: No such
    # file or directory", including the harmless --list call that nextest issues for `--benches`.
    #
    # The version is pinned to the exact patch matching the `gungraun = "=0.19.2"` workspace
    # entry in the root Cargo.toml. gungraun-runner enforces strict string equality on the
    # version (see `gungraun-runner::runner::compare_versions`), so any drift between the
    # installed runner and the library causes `*_cg` benches to fail at runtime with
    # VersionMismatch. Keep this line, the workspace Cargo.toml entry, and the install command
    # in `docs/callgrind-benchmarks.md` in sync when bumping.
    if ($IsLinux) {
        cargo install gungraun-runner --version 0.19.2 --locked
    }

    # azcopy drives the bulk upload in the cargo-bench-history Azure stress harness
    # (`just bench-history-stress-azure`), which is on-demand only and never runs in
    # CI — so, like the Azurite install below, skip the download in CI and only fetch
    # azcopy for local PC setup. It is not a cargo crate and has no installer, so a
    # helper script fetches the pinned official binary; see scripts/install-azcopy.ps1
    # (including its rationale for the destination). No-op when azcopy is already present.
    if ($env:CI) {
        Write-Host "Skipping azcopy install in CI (only the on-demand 'just bench-history-stress-azure' needs it)."
    } else {
        & "{{justfile_directory()}}/scripts/install-azcopy.ps1"
    }

    # Azurite is the Azure Blob storage emulator that backs the cargo-bench-history
    # Azure backend tests, exercised locally via `just test-azurite`. It is a Node
    # package, so npm is a prerequisite (see DEVELOPMENT.md). We install it on every
    # platform because the emulator and those tests are cross-platform. If npm is not
    # present we warn and continue rather than fail, so the rest of the toolchain still
    # installs.
    #
    # The version is pinned to match the CI `start-azurite` action
    # (.github/actions/start-azurite/action.yml) so local and CI runs target the same
    # emulator — keep the two in sync when bumping.
    if ($env:CI) {
        # Skip in CI: `npm install -g azurite` takes ~3 minutes and `install-tools` runs
        # on every CI job, but only the `test-azurite` job needs the emulator — and that
        # job provisions its own copy via the start-azurite action. Installing it here too
        # would add ~3 minutes of uncached work to every other CI job for no benefit.
        Write-Host "Skipping Azurite install in CI (the test-azurite job provisions its own via the start-azurite action)."
    } elseif (Get-Command npm -ErrorAction SilentlyContinue) {
        npm install -g azurite@3.35.0
    } else {
        Write-Warning "npm not found; skipping Azurite install. Install Node.js/npm (see DEVELOPMENT.md), then re-run 'just install-tools' to enable 'just test-azurite'."
    }

    # Pester 5 powers `just test-scripts`, the PowerShell test suite for the standalone scripts
    # under scripts/. GitHub runners preinstall it, and a fresh local PC may only have the
    # Windows-bundled Pester 3, so install a 5.x into the CurrentUser scope when one is not
    # already available. -SkipPublisherCheck sidesteps the signed Windows-bundled Pester 3.
    if (Get-Module -ListAvailable Pester | Where-Object { $_.Version.Major -ge 5 }) {
        Write-Host "Pester 5+ already available; skipping install."
    } else {
        Install-Module Pester -MinimumVersion 5.0 -Scope CurrentUser -Force -SkipPublisherCheck
    }

    # ShellCheck is what actionlint delegates to for linting embedded shell (`run:`) steps. GitHub's
    # Linux runners ship it, so the CI validate-workflows job shellchecks every bash step; installing
    # it here keeps local `just validate-workflows` faithful to CI instead of silently skipping those
    # checks. Like actionlint it is a pinned, checksum-verified standalone binary; see
    # scripts/install-shellcheck.ps1. Installed on every platform with no CI skip. No-op when the
    # pinned ShellCheck is already present.
    & "{{justfile_directory()}}/scripts/install-shellcheck.ps1"

    # actionlint validates the GitHub Actions workflow files (`just validate-workflows`, run both
    # locally and by the CI validate-workflows job). It is not a cargo crate and has no installer, so a
    # helper script fetches the pinned official binary and verifies its checksum; see
    # scripts/install-actionlint.ps1. Unlike azcopy/Azurite this IS needed in CI (the workflow
    # validation job runs it), so it is installed on every platform with no CI skip. No-op when the
    # pinned actionlint is already present.
    & "{{justfile_directory()}}/scripts/install-actionlint.ps1"
