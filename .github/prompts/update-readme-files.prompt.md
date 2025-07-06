---
mode: agent
---
Ensure that the `README.md` in each published package succinctly summarizes
the contents of the package.

A `README.md` is only used for packages that are to be published (`publish = true` in `Cargo.toml`).
Macro packages (anything with `_macros` in the name) do not need a `README.md`, as they only
exist to be hidden dependencies.

The contents of a `README.md` file should be:

1. A summary of no more than 2 pages.
2. A very succinct example.
3. The standard footer.

The example in the readme should have a corresponding `src/examples/readme.rs` file to verify
that it builds and succeeds when executed. If the two are out of sync, adjust the readme file
to match the example in `src/examples/readme.rs`.

The standard footer is:

```
More details in the [package documentation](https://docs.rs/package_name_here/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.
```

The `README.md` at the repo root has links to package-specific `README.md` files. Ensure that
there are no broken links (if a package has no readme file, there is no need to have a link in
the repo root readme file).