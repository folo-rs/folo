[group('basics')]
build PROFILE='dev':
    cargo build {{ target_package }} --profile {{ PROFILE }} --all-features --all-targets --locked

[group('basics')]
check PROFILE='dev':
    cargo +{{ env_var('RUST_MSRV') }} check {{ target_package }} --profile {{ PROFILE }} --all-features --all-targets --locked

# Verify the workspace compiles at the minimum dependency versions our manifests promise.
[group('basics')]
check-frozen:
    # `cargo freeze-deps` rewrites every workspace dependency requirement in Cargo.toml to
    # its declared minimum (`=X.Y.Z`) in place. Regenerating Cargo.lock then makes the
    # resolver select those minimums, and `check` compiles against them — a failure means a
    # published minimum-version promise is too low to actually compile. This rewrites
    # Cargo.toml and Cargo.lock in place, so run it on a throwaway checkout (e.g. CI).
    cargo freeze-deps
    cargo +{{ env_var('RUST_MSRV') }} generate-lockfile
    just package="{{ package }}" check

[group('basics')]
clean:
    cargo clean

[group('basics')]
[script]
docs:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    $env:RUSTDOCFLAGS = "--cfg docsrs -D warnings"
    cargo +$env:RUST_NIGHTLY doc {{ target_package }} --no-deps --all-features --locked

[group('basics')]
[script]
docs-default-features:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    $env:RUSTDOCFLAGS = "--cfg docsrs -D warnings"
    cargo +$env:RUST_NIGHTLY doc {{ target_package }} --no-deps --locked

[group('basics')]
[script]
docs-open:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    $env:RUSTDOCFLAGS = "--cfg docsrs -D warnings"
    cargo +$env:RUST_NIGHTLY doc {{ target_package }} --no-deps --all-features --open --locked
