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

Do not use VS Code tasks, relying instead on `just` and, if necessary, `cargo` commands.

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

# Validate via examples

All stand-alone examples binaries are expected to not panic when executed. When validating changes
to a package, run each stand-alone example binary it has and ensure that none of them panic.

# Extra validation for unsafe code

If a package you are modifying requires unsafe code, you are expected to run the tests under Miri
to confirm that there are no memory safety violations that the compiler alone cannot catch.

# Multiplatform codebase

This is a multiplatform codebase. In some packages you will find folders named `linux` and `windows`,
which contain platform-specific code. When modifying files of one platform, you are also expected
to make the equivalent modifications in the other.

By default, we are operating on Windows. However, you can also invoke commands on Linux using the
syntax `wsl -e bash -l -c "command"`. For example, to test on both Windows and Linux, execute:

1. `just test`
2. `wsl -e bash -l -c "just test"`

You are expected to validate all changes on both operating systems.

# Facades and abstractions

Some packages like `many_cpus` use a platform abstraction layer (PAL), where an abstraction like
`trait Platform` defined in `packages/many_cpus/src/pal/abstractions/**` has multiple different
implementations:

1. A Windows implementation (`packages/many_cpus/src/pal/windows/**`)
2. A Linux implementation (`packages/many_cpus/src/pal/linux/**`)
3. A mock implementation (`packages/many_cpus/src/pal/mocks.rs`)

Logic code will consume this abstraction via facade types, which can either call into the real
implementation of the build target platform (Windows or Linux) or the mock implementation (only
when building in test mode). The facades are defined in `packages/many_cpus/src/pal/facade/**` and
only exist to be minimal pass-through layers to allow swapping in the mock implementation in tests.

When modifying the API of the PAL, you are expected to make the API changes in the
abstraction, facade and implementation types at the same time, as the API surface must match.

The same pattern may also be used elsewhere (e.g. inside the PAL implementations as a second layer
of abstraction, or in other packages).

# Filesystem structure

We prefer many smaller files over few large files, typically only packing implementation details
and unit tests into the same file but keeping separate API-visible types in separate files (even
if only API-visible inside the same crate).

We prefer to keep the public API relatively flat - even if we create separate Rust modules for
types, we re-export them all at the parent, so while we have modules like
`packages/many_cpus/src/hardware_tracker.rs` the type itself is exported at the crate root as
`many_cpus::HardwareTracker` instead of at the module as `many_cpus::hardware_tracker::HardwareTracker`.

# Scripting

You can assume PowerShell 7 (`pwsh`) is available on every operating system and environment.

Prefer PowerShell 7 commands to Bash commands, as they are more likely to work. You
will not always be on Linux, so Bash commands might not always work.

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
calling. The API documentation of an unsafe function has a "Safety" section that poses a challenge
and the safety comment is the response - the two must be paired and correspond to each other.

Safety comments are not there just to re-state the requirements or make generic claims, they must
specifically explain how we satisfy the safety requirements of the function we are calling (e.g.
by referencing an assertion, a type invariant, earlier logic or other mechanism).

Safety comments are also required in examples and doctests that use `unsafe` blocks.

Safety comments (whether single- or multiline) go above the line with the `unsafe` block. To be
clear, they are associated with a specific `unsafe` block, not with a function call. Example:

```rust
/// SAFETY: We ensured above that the pointer is valid and aligned for the type `T`.
unsafe {
    unsafe_function(pointer);
}
```

Each `unsafe` block is expected to only have one call to an unsafe function, and should not have 
nontrivial safe code inside it.

Good example - only unsafe code in the unsafe block:

```rust
/// SAFETY: We ensured above that the pointer is valid and aligned for the type `T`.
let entity = Entity::from(unsafe {
    unsafe_function(pointer)
});
```

Bad example - `unsafe` block includes safe code:

```rust
/// SAFETY: We ensured above that the pointer is valid and aligned for the type `T`.
let entity = unsafe {
    Entity::from(unsafe_function(pointer))
};
```

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

# Comments must add value not re-state the code

Comments that merely restate the obvious are not desired. Comments should add value by explaining
why something is done, what it accomplishes, or how it works. Avoid comments that simply repeat
what the code does.

Bad comment:

```rust
/// Unique identifier for this pool instance.
pool_id: u64,
```

Good comment:

```rust
/// We need to uniquely identify each pool to ensure that memory is not returned to the
/// wrong pool. If the pool ID does not match when returning memory, we panic.
pool_id: u64,
```

# Testing for panics and errors

It is good to create tests that verify expected panics/errors are returned. However, never
check for a specific error/panic message - these are not part of the public API and create
fragile tests. Just verify that an error of an expected type occurs or any panic occurs.

# Keep the house in order

Do not only focus on the immediate task at hand but also consider how it affects the codebase
around it.

If the change you are working on affords more simplicity, better organization or greater reuse,
take action to perform the refactoring needed to achieve that. If the change you are working on
makes some logic or tests obsolete, clean up.

Code should be well covered with unit tests, public APIs should be documented and include inline
examples. The most important scenarios should be covered by stand-alone example binaries.

Performance-critical code should include Criterion benchmarks to help detect regressions.

# Document the contract, not the implementation

API documentation on types and functions should describe the API contract (i.e. the inputs,
the outputs and the behavior) not how it is implemented. Do not discuss implementation details
like private helper types or reference the internal field structure of a type in API documentation.

# Lint suppressions

It is fine to suppress Clippy and compiler lints in the code if it is justified. All suppressions
must have a `reason` field to justify them.

Prefer `expect` over `allow` suppressions, except when applying a broadly-scoped suppression that
applies to a whole file or module using outer attributes, in which case "allow" is preferred.

# Panic in drop()

It is OK to verify that type invariants still hold in `drop()` and panic if some have been violated,
e.g. when an item is not in a valid state to be dropped. However, all such assertions must be
guarded with a `thread::panicking()` check to ensure that the panic does not occur when already
unwinding for another panic - we do not want to double-panic as that mangles the errors.

# Benchmark design

Unless otherwise prompted, create single-threaded synchronous Criterion benchmarks. Use benchmark
groups to group related benchmarks that make sense to compare to each other.

Focus on benchmarking elementary operations, do not create benchmarks with lots of long-winded
logic. We generally want to benchmark a single API call or at most a sequence of closely coupled
API calls.

Only the functionality being benchmarked should be inside the `.iter()` closure, with the data setup
being either done outside (if not per-iteration) or using the first "payload preparation" callback
of `iter_batched()` (if per-iteration).

If multithreaded benchmarks are truly appropriate, use `bench_on_threadpool()` for them. When using
this for multithreaded benchmarks, also run any single-threaded benchmarks
via `bench_on_threadpool()` to ensure that overheads are comparable.

Inside the benchmark closure, use `std::hint::black_box()` to consume output values from the code
being benchmarked, to avoid unrealistic eager optimizations due to output values that are discarded.

# YAML formatting

Prefer not using quotes around strings, unless the string starts with special characters.

Example of desired formatting:

```yaml
regular_field: just some text
special_field: ':::starts with special characters, needs quoting'
```

# Use imports

Types we reference should be imported via `use` statements. Unless there is a specific need
to disambiguate between similarly named types, do not use absolute paths to types.

This is good:

```rust
use std::time::Instant;

fn foo(i: Instant) {}
```

Is is bad:
```rust
fn foo(i: std::time::Instant) { }
```