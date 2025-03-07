[group('setup')]
install-tools:
    cargo install cargo-machete cargo-nextest cargo-release cargo-semver-checks --locked
    rustup toolchain install nightly --component miri
