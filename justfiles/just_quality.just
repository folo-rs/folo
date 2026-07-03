[group('quality')]
clippy PROFILE='dev':
    cargo clippy {{ target_package }} --profile {{ PROFILE }} --all-targets --all-features --locked -- -D warnings

[group('quality')]
[script]
coverage-measure:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    # Before running the tests, we need to clear old test coverage data because the coverage report
    # simply sums up all the data in the target folder, even if it is from old builds.
    cargo +{{ env_var('RUST_NIGHTLY') }} llvm-cov clean --workspace

    # Pre-build the mock benchmark engine once and hand each per-test process its path via
    # MOCK_BENCH_ENGINE. nextest runs every test in its own process, so without this each
    # cargo-bench-history integration test would build the engine on demand and those
    # concurrent `cargo build` invocations would race over the shared coverage target tree
    # (observed as transient "No such file or directory" engine-spawn failures on macOS).
    $env:MOCK_BENCH_ENGINE = just _mock-engine-path | Select-Object -Last 1

    # This will run tests and generate test coverage data files, to be analyzed separately.
    #
    # We enumerate `--lib --bins --tests --examples` instead of using `--all-targets`
    # because `--all-targets` would also pull in `--benches`. Bench targets are deliberately
    # kept out of the coverage pass: they are slow and coverage of benchmark code is not
    # meaningful. The benches are exercised separately (a "does it still build and run"
    # smoke check) through the `test-benches` recipe, which runs them via `cargo test
    # --benches`.
    cargo +{{ env_var('RUST_NIGHTLY') }} llvm-cov nextest {{ target_package }} --no-fail-fast --lib --bins --tests --examples --no-report --all-features --locked

# This tool needs a different way to specify the package.
coverage-package := if package == "" { "" } else { "-p " + replace(package, " ", " -p ") }

[group('quality')]
coverage-report:
    cargo +{{ env_var('RUST_NIGHTLY') }} llvm-cov report {{ coverage-package }} --open

[group('quality')]
coverage-report-ci:
    cargo +{{ env_var('RUST_NIGHTLY') }} llvm-cov report --lcov --output-path lcov.info

[group('quality')]
[no-cd]
format:
    cargo +{{ env_var('RUST_NIGHTLY') }} fmt --verbose --all -- --config-path ./unstable-rustfmt.toml
    cargo sort --workspace
    cargo sort-derives

[group('quality')]
format-check:
    cargo +{{ env_var('RUST_NIGHTLY') }} fmt --verbose --all --check
    cargo sort --workspace --check
    cargo sort-derives --check

# Statically validates the GitHub Actions workflow files under .github/workflows with actionlint
# (installed by `just install-tools`), catching invalid `${{ }}` expressions, unknown `runs-on`
# labels, misuse of `needs`/`if`, deprecated action versions and similar mistakes before they
# reach CI. Run with no arguments, actionlint discovers the workflow files itself. Repo-wide, not
# package-scoped.
[group('quality')]
[script]
validate-workflows:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    actionlint -color

# Validates that every publishable binary crate declares the `[package.metadata.binstall]` block
# the release workflow relies on, so a binary crate can never ship without installable prebuilt
# binaries (see scripts/release/BinstallValidation.psm1 and docs/release-automation.md). Fast and
# repo-wide (not package-scoped); folded into the same CI job as the other script checks.
[group('quality')]
[script]
validate-binstall:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true
    Import-Module ./scripts/release/BinstallValidation.psm1 -Force

    Invoke-BinstallValidation

[group('quality')]
hack:
    cargo hack check {{ target_package }} --feature-powerset --locked

[group('quality')]
machete:
    cargo machete --skip-target-dir
    cargo +{{ env_var('RUST_NIGHTLY') }} udeps
    cargo +{{ env_var('RUST_NIGHTLY') }} udeps --all-targets

[group('quality')]
default-features-check:
    cargo ensure-no-default-features

[group('quality')]
[script]
careful FILTER="":
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    # Pre-build the mock benchmark engine once and pass its path via MOCK_BENCH_ENGINE, so the
    # cargo-bench-history integration tests skip the per-process on-demand engine build (the
    # same pattern the `test` and `coverage-measure` recipes use).
    $env:MOCK_BENCH_ENGINE = just _mock-engine-path | Select-Object -Last 1

    cargo +$env:RUST_NIGHTLY careful test {{ target_package }} --no-fail-fast --all-features {{ FILTER }} --locked

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
    wsl -e bash -l -c "just package='{{ package }}' validate-local"

# Full validation of primary factors, as you would do in a build pipeline before a release.
# Includes package-scoped mutation testing as its final (and by far slowest) step, since
# uncaught mutations are a very common cause of CI failures — we catch them before pushing.
# Performs validation on the current platform, whatever that may be.
[group('quality')]
validate-local:
    just package="{{ package }}" format-check
    just package="{{ package }}" validate-workflows
    just package="{{ package }}" validate-binstall
    just package="{{ package }}" check dev
    just package="{{ package }}" clippy dev
    just package="{{ package }}" test
    just package="{{ package }}" test-docs
    just package="{{ package }}" docs
    just package="{{ package }}" docs-default-features
    just package="{{ package }}" miri
    just package="{{ package }}" machete
    just package="{{ package }}" default-features-check
    just package="{{ package }}" check release
    just package="{{ package }}" clippy release
    just package="{{ package }}" build release
    just package="{{ package }}" mutants

# Validation of extra factors that take potentially too long to run in regular validation.
[group('quality')]
validate-extra: validate-extra-local validate-extra-linux

# Performs the part of the `validate-extra` recipe that must run on Linux, when commanded from Windows.
[group('quality')]
validate-extra-linux:
    wsl -e bash -l -c "just package='{{ package }}' validate-extra-local"

# Validation of extra factors that take potentially too long to run in regular validation.
# Mutation testing is intentionally not here — it runs as part of `validate-local`.
# Performs validation on the current platform, whatever that may be.
[group('quality')]
validate-extra-local:
    just package="{{ package }}" run-examples
    just package="{{ package }}" hack
    just package="{{ package }}" careful
