[group('setup')]
install-tools:
    cargo install cargo-machete cargo-nextest release-plz cargo-semver-checks cargo-audit cargo-hack --locked
    rustup toolchain install nightly --component miri
