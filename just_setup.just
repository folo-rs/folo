[group('setup')]
install-tools:
    cargo install cargo-machete cargo-nextest release-plz cargo-semver-checks --locked
    rustup toolchain install nightly --component miri
