build PROFILE='dev':
    cargo build --workspace --profile {{ PROFILE }} --all-features --all-targets

clean:
    cargo clean

docs:
    cargo doc --workspace --no-deps --all-features

docs-open:
    cargo doc --workspace --no-deps --all-features --open
