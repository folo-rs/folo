[group('setup')]
install-tools:
    rustup toolchain install nightly --component miri --component rustfmt --component rust-src
    rustup toolchain install {{ CARGO_CHECK_EXTERNAL_TYPES_TOOLCHAIN }}
    cargo install cargo-machete cargo-nextest release-plz cargo-semver-checks cargo-audit cargo-hack cargo-mutants cargo-llvm-cov cargo-sort cargo-sort-derives cargo-udeps cargo-careful cargo-check-external-types --locked
