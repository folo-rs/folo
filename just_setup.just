[group('setup')]
install-tools:
    rustup toolchain install nightly --component miri --component rustfmt --component rust-src
    cargo install cargo-machete cargo-nextest release-plz cargo-semver-checks cargo-audit cargo-hack cargo-mutants cargo-llvm-cov cargo-sort-derives cargo-udeps cargo-careful --locked
