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

    cargo install cargo-machete cargo-nextest release-plz cargo-semver-checks cargo-audit cargo-hack cargo-mutants cargo-llvm-cov cargo-sort cargo-sort-derives cargo-udeps cargo-careful cargo-ensure-no-default-features --locked
