[group('quality')]
clippy PROFILE='dev':
    cargo clippy {{ target_package }} --profile {{ PROFILE }} --all-targets --all-features --locked -- -D warnings

[group('quality')]
coverage-measure:
    # Before running the tests, we need to clear old test coverage data because the coverage report
    # simply sums up all the data in the target folder, even if it is from old builds.
    cargo llvm-cov clean --workspace

    # This will run tests and generate test coverage data files, to be analyzed separately.
    cargo llvm-cov nextest {{ target_package }} --all-targets --no-report --all-features --locked

# This tool needs a different way to specify the package.
coverage-package := if package == "" { "" } else { "-p " + package }

[group('quality')]
coverage-report:
    cargo llvm-cov report {{ coverage-package }} --open

[group('quality')]
format:
    cargo fmt --verbose --all

[group('quality')]
format-check:
    cargo fmt --verbose --all --check

[group('quality')]
hack:
    cargo hack check --feature-powerset

[group('quality')]
machete:
    cargo machete --skip-target-dir

# Separate file because it is a giant script.
import 'just_quality_mutants.just'

# Full validation of primary factors, as you would do in a build pipeline before a release.
# Skips some potentially very lengthy validation, which you can run separately via `validate-extra`.
# We assume this is executed on Windows, and will also perform the full validation on Linux.
[group('quality')]
validate: validate-local validate-linux

# Performs the part of the `validate` recipe that must run on Linux, when commanded from Windows.
[group('quality')]
validate-linux:
    wsl -e bash -l -c "just validate-local"

# Full validation of primary factors, as you would do in a build pipeline before a release.
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

# Validation of extra factors that take potentially too long to run in regular validation.
[group('quality')]
validate-extra: validate-extra-local validate-extra-linux

# Performs the part of the `validate-extra` recipe that must run on Linux, when commanded from Windows.
[group('quality')]
validate-extra-linux:
    wsl -e bash -l -c "just validate-extra-local"

# Validation of extra factors that take potentially too long to run in regular validation.
# Performs validation on the current platform, whatever that may be.
[group('quality')]
validate-extra-local: mutants
