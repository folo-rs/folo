[group('setup')]
install-tools:
    cargo install cargo-machete cargo-nextest release-plz cargo-semver-checks cargo-audit cargo-hack cargo-mutants --locked
    rustup toolchain install nightly --component miri
