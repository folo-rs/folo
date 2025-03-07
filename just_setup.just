[group('setup')]
install-tools:
    cargo install cargo-machete cargo-nextest cargo-release --locked
    rustup toolchain install nightly --component miri
