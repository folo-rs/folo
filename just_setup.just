[group('setup')]
install-tools:
    cargo install cargo-machete cargo-nextest release-plz cargo-semver-checks cargo-audit cargo-hack cargo-mutants cargo-llvm-cov cargo-sort-derives --locked --force
    rustup toolchain install nightly --component miri
