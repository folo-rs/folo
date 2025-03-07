[group('quality')]
check PROFILE='dev':
    cargo check {{ target_package }} --profile {{ PROFILE }} --all-features --all-targets

[group('checks')]
clippy:
    cargo clippy {{ target_package }} --all-targets --all-features --locked -- -D warnings

[group('quality')]
format:
    cargo fmt --verbose --all

[group('quality')]
format-check:
    cargo fmt --verbose --all --check

[group('quality')]
format-self:
    just --fmt --unstable

[group('quality')]
machete:
    cargo machete --skip-target-dir

# Full validation of everything, as you would do in a build pipeline before a release.
[group('quality')]
validate: machete format-check clippy check test test-docs docs miri