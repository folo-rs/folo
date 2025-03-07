[group('setup')]
install-tools:
    cargo install cargo-machete cargo-nextest --locked
    rustup toolchain install nightly --component miri
