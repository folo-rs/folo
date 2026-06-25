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

    cargo install cargo-machete cargo-nextest release-plz cargo-semver-checks cargo-audit cargo-hack cargo-llvm-cov cargo-sort cargo-sort-derives cargo-udeps cargo-careful cargo-ensure-no-default-features cargo-delta --locked

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
    # (`just bench-history-stress-azure`). It is not a cargo crate, so fetch the
    # official static binary for this OS and drop it next to the cargo tools (already
    # on PATH). Skipped when azcopy is already available.
    if (Get-Command azcopy -ErrorAction SilentlyContinue) {
        Write-Host "azcopy already installed; skipping."
    } else {
        Write-Host "Installing azcopy (used by 'just bench-history-stress-azure')..."

        # Land the binary in the cargo bin directory, which is already on PATH for
        # every other tool installed by this recipe.
        $dest = Join-Path $HOME ".cargo/bin"
        New-Item -ItemType Directory -Force -Path $dest | Out-Null

        # Stage the download in a throwaway temp directory so a failed extract
        # leaves nothing behind.
        $work = New-Item -ItemType Directory -Force -Path (Join-Path ([System.IO.Path]::GetTempPath()) ("azcopy-" + [guid]::NewGuid()))
        try {
            # `$IsWindows` is undefined on Windows PowerShell 5.1, where we are on
            # Windows by definition, so treat "undefined" as Windows too.
            if ($IsMacOS) {
                Write-Host "Downloading azcopy for macOS..."
                $archive = Join-Path $work "azcopy.zip"
                Invoke-WebRequest -Uri "https://aka.ms/downloadazcopy-v10-mac" -OutFile $archive
                Expand-Archive -Path $archive -DestinationPath $work -Force
                $bin = Get-ChildItem -Path $work -Recurse -Filter "azcopy" | Select-Object -First 1
            } elseif ($IsLinux) {
                Write-Host "Downloading azcopy for Linux..."
                $archive = Join-Path $work "azcopy.tar.gz"
                Invoke-WebRequest -Uri "https://aka.ms/downloadazcopy-v10-linux" -OutFile $archive
                tar -xf $archive -C $work
                $bin = Get-ChildItem -Path $work -Recurse -Filter "azcopy" | Select-Object -First 1
            } else {
                Write-Host "Downloading azcopy for Windows..."
                $archive = Join-Path $work "azcopy.zip"
                Invoke-WebRequest -Uri "https://aka.ms/downloadazcopy-v10-windows" -OutFile $archive
                Expand-Archive -Path $archive -DestinationPath $work -Force
                $bin = Get-ChildItem -Path $work -Recurse -Filter "azcopy.exe" | Select-Object -First 1
            }

            if ($null -eq $bin) {
                throw "azcopy binary was not found inside the downloaded archive."
            }

            Copy-Item -Path $bin.FullName -Destination $dest -Force
            Write-Host "Installed azcopy to $dest."
        } finally {
            Remove-Item -Path $work -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
