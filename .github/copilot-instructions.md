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

After making changes to the codebase, you are expected to validate the essentials:

* `just format` to apply auto-formatting to code files, ensuring consistent code style.
* `just check` to verify that compiler checks pass.
* `just clippy` to verify that all linter rules pass.
* `just test` to verify that the code compiles and tests pass.
* `just test-docs` to verify that doctests pass.
* `just docs` to verify that the API documentation builds without errors.

You can run all of these validation steps in one go via `just validate-quick`. This tends to
be faster than doing the individual validation steps one by one.

We operate under a "zero warnings allowed" requirement - fix all warnings that validation generates.

# Extra validation for unsafe code

If a package you are modifying requires unsafe code, you are expected to run the tests under Miri
to confirm that there are no memory safety violations that the compiler alone cannot catch.

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

You can assume PowerShell 7 (`pwsh`) is available. Prefer PowerShell 7 over Bash.

# Code style

We limit line length to 100, including API documentation and comments.

There are many Clippy rules defined in the workspace-level `Cargo.toml`.

Follow these even in doctests. Note that Clippy does not actually check doctests! You will need to
manually check what Clippy rules we enable in the workspace-level `Cargo.toml` and follow them in
the inline examples in API documentation.

# Language

Use proper English grammar, spelling and punctuation.

Sentences end with punctuation:

* This is wrong: "//! // Create a pool for storing u64 values"
* This is correct: "//! Create a pool for storing u64 values."

Do not forget proper language in tests, doctests and examples.

# Use of unwrap() and expect()

Only use `unwrap()` in test code.

You may use `expect()` in non-test code but only if there is a reason to believe that the expectation
will never fail. That is, we do not use `expect()` as an assertion, we use it to cut off unreachable
code paths. The message inside `expect()` should explain why we can be certain that code path is
unreachable - it is not an error message saying what went wrong!

Using `assert!()` or other panic-inducing macros in non-test code is fine as long as it is documented
in API documentation (in a `# Panics` section).

# Safety comments

Safety comments must explain how we satisfy the safety requirements of the unsafe function we are
calling. Safety comments are not there just to re-state the requirements, they must explain how
we satisfy them (e.g. by referencing an assertion, a type invariant, earlier logic or other mechanism).

# Whitespace

There should be an empty line between functions.

# Non-zero integers

Whenever a numeric value must be non-zero, prefer `NonZero<usize>` over `usize`,
in both private APIs/logic and public APIs. Prefer `NonZero<usize>` over `NonZeroUsize`.

# File contents flow

Bigger and more important types go higher in the file, smaller and less important types go lower.

Public API types go higher in the file, private types go lower.

The implementation of a type should stay close to the definition of the type (e.g. `impl` blocks
of a type follow the `struct` block).

# Visibility

Types, fields, functions and methods should have the minimum required visibility, only being
visible in the same file by default if not part of the public API surface.

Use `pub(crate)` only when a type actually needs to be accessible in other files of the same crate.

# Diagrams

Mermaid diagrams are encouraged in API documentation where they make sense.

Each diagram should be a separate file in a `crate/docs/diagrams` folder, with the file
extension `.mermaid`. The file name should match the diagram name used in the documentation.

Rendering Mermaid diagrams in Rust API documentation requires the `simple-mermaid` package to
be referenced. The syntax for embedding a Mermaid diagram is
`#![doc = mermaid!("../doc/region_cached.mermaid")]`. See existing examples for a detailed
reference (e.g. the `region_local` package).
