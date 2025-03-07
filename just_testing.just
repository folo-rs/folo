[group('testing')]
bench TARGET="":
    #!{{ shebang }}
    $target_selector = @()

    if ("{{ TARGET }}" -ne "") {
        $target_selector += "--bench"
        $target_selector += "{{ TARGET }}"
    }

    cargo bench {{ target_package }} --all-features $target_selector

[group('testing')]
miri:
    cargo +nightly miri nextest run {{ target_package }}

[group('testing')]
test FILTER="":
    cargo nextest run {{ target_package }} --all-targets --all-features {{ FILTER }}

[group('testing')]
test-docs FILTER="":
    cargo test {{ target_package }} --all-features --doc {{ FILTER }}
