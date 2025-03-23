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
    #!{{ shebang }}

    $args = @(
        "-e"
        # Parts of this crate require Criterion to work and other parts are currently not tested
        # as there is no public way to simulate a system topology for `many_cpus`.
        "many_cpus_benchmarking",

        "-e"
        # Macros are tested via the impl crate, mutations in the middle layer might not be detected.
        "linked_macros"

        # We do not test facades, as they are just trivial code that forwards calls to real impls.
        "-e"
        "**/*facade.rs"
        "-e"
        "facade"

        "-e"
        # We have limited coverage of platform bindings because it can be difficult to set up the
        # right scenarios for each, given they are platform-dependent. Instead, we test higher
        # level code using a mock platform.
        "bindings"
    )

    if ($IsLinux) {
        $args += "-e"
        $args += "**/*windows.rs"

        $args += "-e"
        $args += "windows"
    } else {
        $args += "-e"
        $args += "**/*linux.rs"

        $args += "-e"
        $args += "linux"
    }

    # We deliberately do not use nextest here because it cannot run doctests.
    cargo mutants {{ target_package }} --profile=mutants @args

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
