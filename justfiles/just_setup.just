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

    cargo install cargo-machete cargo-nextest release-plz cargo-semver-checks cargo-audit cargo-hack cargo-mutants cargo-llvm-cov cargo-sort cargo-sort-derives cargo-udeps cargo-careful cargo-ensure-no-default-features cargo-delta --locked

    # gungraun-runner drives Valgrind for cycle-accurate benchmarks. Only useful on Linux
    # (Valgrind is Linux-only); on Windows and macOS the cycle bench binaries are no-op stubs.
    #
    # The cycle bench binary is just a shim that forwards its CLI args (including the libtest
    # --list / --format / --ignored flags that `cargo bench` and `nextest` use during target
    # enumeration) to gungraun-runner via an encoded payload. Without gungraun-runner on $PATH,
    # any invocation of a `*_cycles` bench binary fails with "Failed to run benchmarks: No such
    # file or directory", including the harmless --list call that nextest issues for `--benches`.
    #
    # The version is pinned to the exact patch matching the `gungraun = "=0.19.0"` workspace
    # entry in the root Cargo.toml. gungraun-runner enforces strict string equality on the
    # version (see `gungraun-runner::runner::compare_versions`), so any drift between the
    # installed runner and the library causes `*_cycles` benches to fail at runtime with
    # VersionMismatch. Keep this line, the workspace Cargo.toml entry, and the install command
    # in `docs/cycle-accurate-benchmarks.md` in sync when bumping.
    if ($IsLinux) {
        cargo install gungraun-runner --version 0.19.0 --locked
    }
