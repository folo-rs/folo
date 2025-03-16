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
    cargo nextest run {{ target_package }} --all-features {{ FILTER }}

# We run benches separately because they are slow in Nextest multi-process mode,
# probably due to the Gnuplot integration that spawns an external process.
[group('testing')]
test-benches FILTER="":
    cargo test --benches --all-features {{ FILTER }}

[group('testing')]
test-docs FILTER="":
    cargo test {{ target_package }} --all-features --doc {{ FILTER }}
