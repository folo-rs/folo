[group('setup')]
install-tools:
    # For local development only.
    # CI uses taiki-e/install-action for pre-built binaries - keep tool list in sync with
    # .github/actions/setup-environment/action.yml
    rustup toolchain install nightly --component miri --component rustfmt --component rust-src --component llvm-tools-preview
    cargo install cargo-machete cargo-nextest release-plz cargo-semver-checks cargo-audit cargo-hack cargo-mutants cargo-llvm-cov cargo-sort cargo-sort-derives cargo-udeps cargo-careful cargo-ensure-no-default-features --locked
