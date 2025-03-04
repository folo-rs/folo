format:
    cargo fmt --verbose --all

format-check:
    cargo fmt --verbose --all --check

format-self:
    just --fmt --unstable

machete:
    cargo machete --skip-target-dir
