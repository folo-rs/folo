# Standard commands

We use the Just command runner for many common commands - look inside *.just files to see the
list of available commands. Some relevant ones are:

* `just build` - build the entire workspace
* `just package=many_cpus build` - build a single package (most commands accept a `package` parameter)
* `just test` - test the entire workspace; this does NOT run doctests, use `just test-docs` for that
* `just docs` - build API documentation

The `package` argument must be the first argument to any `just` command, if used.

Avoid running `just bench`, as the benchmarks take a lot of time and `just test` will anyway run
a single benchmark iteration to validate they are still working.

We generally prefer using Just commands over raw Cargo commands if there is a suitable Just command
defined in one of the *.just files.

Do not execute `just release` - this is a critical tool reserved for human use.

# Validating changes

After making changes to the codebase, you are expected to validate the basics.

Use `just test` to verify that the code compiles and tests pass.

Use `just test-docs` to verify that doctests pass.

Use `just docs` to verify that the API documentation builds without errors.

Use `just clippy` to verify that all linter rules pass. We operate under a "zero warnings allowed"
requirement - fix all warnings that Clippy generates.

Use `just format` to apply auto-formatting to code files, ensuring consistent code style.

# Multiplatform codebase

This is a multiplatform codebase. In some crates you will find folders named `linux` and `windows`,
which contain platform-specific code. When modifying files of one platform, you are also expected
to make the equivalent modifications in the other.

By default, we are operating on Windows. However, you can also invoke commands on Linux using the
syntax `wsl -e bash -l -c "command"`. For example, to test on both Windows and Linux, execute:

1. `just test`
2. `wsl -e bash -l -c "just test"`

You are expected to validate all changes on both operating systems.

# Facades and abstractions

Some crates like `many_cpus` use a platform abstraction layer (PAL), where an abstraction like
`trait Platform` defined in `crates/many_cpus/src/pal/abstractions/**` has multiple different
implementations:

1. A Windows implementation (`crates/many_cpus/src/pal/windows/**`)
2. A Linux implementation (`crates/many_cpus/src/pal/linux/**`)
3. A mock implementation (`crates/many_cpus/src/pal/mocks.rs`)

Logic code will consume this abstraction via facade types, which can either call into the real
implementation of the build target platform (Windows or Linux) or the mock implementation (only
when building in test mode). The facades are defined in `crates/many_cpus/src/pal/facade/**` and
only exist to be minimal pass-through layers to allow swapping in the mock implementation in tests.

When modifying the API of the PAL, you are expected to make the API changes in the
abstraction, facade and implementation types at the same time, as the API surface must match.

The same pattern may also be used elsewhere (e.g. inside the PAL implementations as a second layer
of abstraction, or in other crates).

# Filesystem structure

We prefer many smaller files over few large files, typically only packing implementation details
and unit tests into the same file but keeping separate API-visible types in separate files (even
if only API-visible inside the same crate).

We prefer to keep the public API relatively flat - even if we create separate Rust modules for
types, we re-export them all at the parent, so while we have modules like
`crates/many_cpus/src/hardware_tracker.rs` the type itself is exported at the crate root as
`many_cpus::HardwareTracker` instead of at the module as `many_cpus::hardware_tracker::HardwareTracker`.

# Scripting

You can assume PowerShell is available. Prefer PowerShell over Bash.

# Code style

There are many Clippy rules defined in `./Cargo.toml`. Try to follow these even in doctests.
Note that Clippy does not actually run on doctests, so you will need to manually check what
rules we enable and try your best to follow them in the inline examples in API documentation.

# Language

Use proper English grammar, spelling and punctuation. Sentences end with punctuation.
