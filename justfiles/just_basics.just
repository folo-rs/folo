[group('basics')]
build PROFILE='dev':
    cargo build {{ target_package }} --profile {{ PROFILE }} --all-features --all-targets --locked

[group('basics')]
check PROFILE='dev':
    cargo check {{ target_package }} --profile {{ PROFILE }} --all-features --all-targets --locked

[group('basics')]
clean:
    cargo clean

[group('basics')]
[script]
docs:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    $env:RUSTDOCFLAGS = "-D warnings"
    cargo doc {{ target_package }} --no-deps --all-features --locked

[group('basics')]
[script]
docs-open:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    $env:RUSTDOCFLAGS = "-D warnings"
    cargo doc {{ target_package }} --no-deps --all-features --open --locked
