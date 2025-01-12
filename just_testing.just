bench:
    cargo bench --all-features

miri:
    cargo +nightly miri nextest run
    cargo +nightly miri nextest run

test:
    cargo nextest run --workspace --all-targets --all-features

test-docs:
    cargo test --workspace --all-features --doc
