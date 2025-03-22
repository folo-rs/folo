[group('release')]
audit:
    cargo audit

[group('quality')]
check PROFILE='dev':
    cargo check {{ target_package }} --profile {{ PROFILE }} --all-features --all-targets

[group('checks')]
clippy PROFILE='dev':
    cargo clippy {{ target_package }} --profile {{ PROFILE }} --all-targets --all-features --locked -- -D warnings

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
hack:
    cargo hack check --feature-powerset

[group('quality')]
machete:
    cargo machete --skip-target-dir

[group('quality')]
mutants:
    cargo mutants

# Full validation of everything, as you would do in a build pipeline before a release.
# We assume this is executed on Windows, and will also perform the full validation on Linux.
[group('quality')]
validate: validate-local validate-linux

# Performs the part of the `validate` recipe that must run on Linux, when commanded from Windows.
[group('quality')]
validate-linux:
    wsl -e bash -l -c "just validate-local"

# Full validation of everything, as you would do in a build pipeline before a release.
# Performs validation on the current platform, whatever that may be.
[group('quality')]
validate-local:
    cargo generate-lockfile
    just machete
    just format-check
    just clippy dev
    just check dev
    just test
    just test-docs
    just test-benches
    just docs
    just miri
    just clippy release
    just check release
    just build release
    just hack
