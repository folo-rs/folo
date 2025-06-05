[group('basics')]
build PROFILE='dev':
    cargo build {{ target_package }} --profile {{ PROFILE }} --all-features --all-targets

[group('basics')]
check PROFILE='dev':
    cargo check {{ target_package }} --profile {{ PROFILE }} --all-features --all-targets

[group('basics')]
clean:
    cargo clean

[group('basics')]
[script]
docs:
    $env:RUSTDOCFLAGS = "-D warnings"
    cargo doc {{ target_package }} --no-deps --all-features

[group('basics')]
[script]
docs-open:
    $env:RUSTDOCFLAGS = "-D warnings"
    cargo doc {{ target_package }} --no-deps --all-features --open
