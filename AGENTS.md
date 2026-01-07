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

Validate changes via `just validate-local`. This runs a number of different checks and will
uncover most issues.

We operate under a "zero warnings allowed" requirement - fix all warnings that validation generates.

# Multiplatform codebase

This is a multiplatform codebase. In some packages you will find folders named `linux` and
`windows`, which contain platform-specific code. When modifying files of one platform, you
make the equivalent modifications in the other.

When running on a local PC, the operating systme is Windows. When running in GitHub, the operating
system is Linux.

On local PC, you can invoke any Linux commands using the syntax `wsl -e bash -l -c "command"`.
For example, to run the standard validation on both Windows and Linux, execute:

1. `just validate-local`
2. `wsl -e bash -l -c "just validate-local"`

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

Titles are normal sentences, do not capitalize every word in a title.

Sentences end with punctuation:

* This is wrong: "//! // Create a pool for storing u64 values"
* This is correct: "//! Create a pool for storing u64 values."

Comments are regular sentences and END WITH A PERIOD or other punctuation.

Do not forget proper language in tests, doctests and examples.

Do not use contractions, that is not professional English. Use "do not" instead of "don't", etc.

# Use of unwrap() and expect()

Use `unwrap()` in test code and only in test code. Do not use `expect()` in test code.

You may use `expect()` in non-test code but only if there is a reason to believe that the expectation
will never fail. That is, we do not use `expect()` as an assertion, we use it to cut off unreachable
code paths. The message inside `expect()` should explain why we can be certain that code path is
unreachable - it is not an error message saying what went wrong!

State clearly in the `expect()` message why the expectation is guaranteed to hold. Do not use words
like "should" - if it only "should" hold, then you have failed to establish a guarantee!

Using `assert!()` or other panic-inducing macros in non-test code is fine as long as it is documented
in API documentation (in a `# Panics` section). Treat the `assert!()` message the same as the
message for `expect!()` - it should justify why we expect the assertion to hold. If we do not
expect the assertion to hold but are merely fulfilling an API contract to panic, no assert message
is to be used. Similarly, do not use assertion messages in tests.

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

Benchmarks that are meant to be compared to each other must be in the same benchmark function
and in the same benchmark group.

Do not forget to register benchmarks in `Cargo.toml`.

# YAML formatting

Prefer not using quotes around strings, unless the string starts with special characters.

Example of desired formatting:

```yaml
regular_field: just some text
special_field: ':::starts with special characters, needs quoting'
```

# Use imports and do not reference types via absolute paths

Types we reference should be imported via `use` statements. Unless there is a specific need
to disambiguate between similarly named types, do not use absolute paths to types.

This is good:

```rust
use std::time::Instant;

fn foo(i: Instant) {}
```

This is bad:

```rust
fn foo(i: std::time::Instant) { }
```

Referencing types in `std` via absolute paths for no reason is especially sinful.

It is fine to rely on the Rust prelude - no need to explicitly import types from the Rust prelude.

`use` statements go at the top of the file or module, not inside functions.

Do not `use super::` except in unit tests. Instead, use the full path `use crate::` style.

Prefer importing types at the highest level they are visible. For example, if a type is defined
in `src/session.rs` but also re-exported at the crate root, you should import it from the crate
root.

Wrong:

```rust
use crate::session::AllocationTrackingSession;
```

Correct:

```rust
use crate::AllocationTrackingSession;
```

Exceptions:

* Refer to `std::alloc::System` by full path because the short form of this causes AI hallucinations.

# Dependencies in cargo.toml

Dependencies are sorted alphabetically by name of the package.

# Do not use `\n` in println!() statements

To empty an empty line to the terminal, use use `println!();` instead of
embedding `\n` in another printed line.

# Delays are forbidden in test code

There should never be any delay/sleep on the successful path in tests - every test must be
near-instantaneous and time-based synchronization is forbidden.

To be clear, any form of "sleep" or "delay" that uses a real-time clock is forbidden in test code.
If the type can be made to use a `tick::Clock` then a clock using a simulated time source may be
used in test code (via `tick::ClockControl`).

You may use events/signals for synchronization (e.g. `Barrier` or `events_once` events or message channels),
as long as there are no delays or wait-loops in the test code itself.

# Tests must not hang

When there is a danger that a test may hang (e.g. it contains a `.wait()`, `.recv()` or
similar call), you must use a watchdog timer to force a timeout after 10 seconds.

# Named constants

Avoid magic values in the code and use named constants instead. It does not matter how many
times the magic value is present, even one instance is enough to warrant a named constant.

If constants are only used in one function, put them at the top of that function.

The exception is example code - if a magic value is only used once in an example, it is
fine to leave it inline.

# Design documentation

Document design elements, key decisions and architectural choices in inline comments in the
files to which they apply. Use regular `//` comments, not API documentation comments.

# Hide async entrypoint in examples

In inline code examples that use `.await`, do not render the async
entrypoint/wrapper in generated API documentation.

Example:

```rust
use events::once::Event;
# use futures::executor::block_on;

# block_on(async {
let event = get_event();
let message = event.await;

assert_eq!(message, "Hello, World!");
# });
```

# Re-export defaults to wildcard

We prefer to re-export public types from private modules
using a wildcard import. There is no need to name the types separately.

Good:

```rust
pub use events::*;
```

Bad:

```rust
pub use events::{Event, LocalEvent, EventPool, LocalEventPool};
```

# Prefer pub(crate) over pub(other)

We do not use `pub(super)` and other fine-grained visibility modifiers. A type is either private
to a module, public, or public within the whole crate.

# Checked arithmetic

Unless there is a specific reason to use saturating/wrapping arithmetic, use checked arithmetic
(`.checked_add()` and similar) and handle the error case. Do not use regular unchecked arithmetic
(`+`, `-`, `*`, `/`, `%`) as it can overflow and panic.

It is fine to `.expect()` success if there is some reason to believe overflow can never happen,
e.g. because it is guarded by an assertion or because it would require some data structure to
exceed the size of virtual memory. If very confident that an overflow can never occur, it is
fine to use wrapping arithmetic via explicit `.wrapping_add()` methods.

This only applies to non-test code - in tests and benchmarks, it is fine to use whatever arithmetic
is most convenient.

# Examples show good practices

Examples are production code. Use proper patterns and practices - examples are not tests, where
looser rules can be allowed.

# Inline examples separate scenarios into separate code blocks

If you create inline (doctest) examples that showcase multiple scenarios/variations, separate each
variant into its own code block with a short individual description instead of showing examples of
multiple scenarios in one code block.

# Stand-alone examples separate scenarios into functions

If you create stand-alone example files that showcase multiple scenarios/variations, separate each
variant into its own function instead of having everything in a giant `main()` function.

# Replacing text in code files

Prefer applying diffs over generating regex-replace commands for the terminal.

# Avoid the `pin!` macro in tests and examples

This macro is special-purpose and not intended for general use. Instead, use
`Box::pin(value)` to pin a value in examples.

If there is some reason `Box::pin(value)` would not work, you can use `std::pin::pin!(value)`
as a last resort but leave a comment to justify why this is the case.

# Multiple statements per command

You can execute multiple statements per command in the terminal, separated by semicolons. Do not
waste time executing one command at a time when you can execute multiple commands in one go.

```powershell
mv src/old.rs src/new.rs; cargo fmt; cargo clippy --fix --allow-dirty --allow-staged; cargo test
```

# Examples for README.md files

In each package with a `README.md` file, there should be a corresponding
`examples/package_name_readme.rs` file that contains the Rust code present in the example.
This is important to verify that the example actually works.

If the two are out of sync, use the `package_name_readme.rs` as the authoritative source
and update the `README.md` file to match it.

It is fine to disable Clippy rules in the `package_name_readme.rs` file, as it
is not production code and often needs to take shortcuts to be short and simple.

# Memory allocation is the root of all evil

Avoid algorithms that allocate memory at runtime when an allocation-free alternative is available.

# Miri and platform compatibility

Tests that talk to the real operating system generally fail to execute under Miri. This is fine and
expected. However, a Miri test run must still succeed with a clean result! If there are tests that
cannot be executed under Miri, they should be excluded via `#[cfg_attr(miri, ignore))]` (plus a comment
justifying why it is correct to exclude them).

Naturally, if it is possible to redesign a test so it does not rely on the operating system, that
is even better. However, this is not always possible.

Miri is too slow when running tests with large data sets (anything with 100s or 1000s of items).
Exclude such tests from running under Miri.

# Mutation testing coverage and skipping mutations

We expect all mutations to either be unviable or to be caught. Uncaught mutations and mutants that
time out are anomalies that must be corrected.

It is acceptable to skip mutations if they are impractical to test. Some justifiable reasons are:

* Detecting the mutation requires real timing logic to be used. We intentionally do not permit any
  timing-dependent code in our test cases. However, if circumstances allow, we can use the `tick`
  crate with its simulated clock to create timing-dependent logic that can be tested without real
  time passing.
* Detecting the mutation requires detecting that a thing is not happening (e.g. detecting that an object
  is never dropped or that some code never executes). This can sometimes be impossible without relying
  on real-time timeouts (which would violate the above expectation).
* The mutation is in a defensive branch for defense in depth and can never be reached due to higher layers
  of the API preventing the situation from arising.
* The mutation is in trivial forwarder code (e.g. a facade that chooses between a real and mock implementation).

To skip a mutation, use the `#[cfg_attr(test, mutants::skip)]` style and leave a comment to justify
why we are skipping it.

Before skipping a mutation, consider how to catch it. Beyond simply improving test coverage, the
following techniques may help:

* Adding `debug_assert!()` statements to strategic places, verifying that logical invariants still hold.

# Adjust patterns, fix entire classes of problems

If you are asked to do a thing to one instance of a problem in a file, check for other instances.
You must solve the entire class of problems at once, not expect each instance to be pointed
out to you in instructions.

# Documentation is about today, not about yesterday

Both inline and API documentation must describe the current facts, not history from previous
designs or iterations. Do not make comparisons with mechanisms that no longer exist in the API.

# Test-only code requires cfg(test)

If there are functions that are only used in tests, mark them (and their `use` statements)
with `#[cfg(test)]`. Do not just suppress "dead code" warnings.

# Feature-gated code should also be enabled by `test` build

If code is feature-grated, it should always also be enabled in test builds: `#[cfg(any(test, feature = "foo"))]`

# There are many files in the workspace

Avoid running large "process all files" commands directly in the workspace root. Use at least
one level of subdirectory.

# Dev-dependencies within workspace packages must be path dependencies

Do not use `version = "1.2.3"` or `workspace = true` when adding a package from the same workspace as a dev-dependency. Within the same workspace, dev-dependencies must always be `path = "../foo"` style path-references.

# Test presence of auto traits via static assertions

Use `static_assertions::assert_impl_all!` and `static_assertions::assert_not_impl_any!` to check
for the presence or absence of auto traits where the API documentation makes claims about them,
for example in terms of being thread-mobile (Send + !Sync), thread-safe (Send + Sync)
or single-threaded (neither).

If generic type parameters are involved, these assertions should use some randomly selected typical
types that may be encountered in user code.

# Macros may need public-private visibility

For purpose of accessing members of types or modules from published macros, it is permissible to
make logically private items public. This must be accompanied by two adjustments:

* The item must be marked with `#[doc(hidden)]` to prevent it from being included in the
  public API documentation. This is our signal that it is not an officially supported public API.
* The path to the item must include the keyword "private". The preferred forms are:
  - For types, use a `private` module, e.g. `foo::private::Bar`.
  - For methods, use a `__private_` prefix, e.g. `Bar::__private_reset_counters()`.

# UI tests

UI tests all go in a workspace-scoped `ui_tests` package due to technical limitations. Follow
inline documentation in this package to understand more.

Package dependencies of UI tests must be excluded from `udeps` scanner logic via `Cargo.toml` in
`ui_tests`. See existing examples in this file.

# Do not check for specific panic or error messages

Tests that use `#[should_panic]` or use `Display` output of error types must not check for specific
panic or error messages - these messages are not an API contract and may change at any time.

This only pertains to error messages - non-error outputs of `Display` should still be tested.

# Type names

Do not hardcode type names in string literals. Instead use `type_name::<Self>()` or similar.

# PowerShell scripts in justfiles must consider exit codes

Every PowerShell script in a .just file (i.e. every `[script]` block) must start with the following:

```powershell
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
```

This ensures that commands that produce nonzero exit codes are correctly considered errors and fail the script.

# Test coverage

In some circumstances, we may need to mark parts of code as intentionally not covered by tests. For example:

* Tests themselves - the tooling measures test code as well as real code. We only care about real code, so all
  unit test modules and mock/fake test utilities (anything `#[cfg(test)]`) need to be excluded. Integration tests
  in `tests/` are automatically excluded, though - no need to worry about those.
* Defensive branches that can never be reached due to defense in depth layering.
* Code that is only ever executed on a const context, as const context is not covered in coverage measurements.
* When code has no API contract to test (e.g. `fmt::Debug` implementations which may contractually write anything).
* Facade types whose only purpose is to redirect calls to either a real or mock implementation - not worth testing.

To exclude code from coverage measurement, mark it with `#[cfg_attr(coverage_nightly, coverage(off))]`. This
also requires `#![cfg_attr(coverage_nightly, feature(coverage_attribute))]` on the crate level.

When excluding code for any other reason than "it is test code", leave a comment to explain why.

# Keep names simple and unadorned

Avoid unnecessary and repetitive prefixes and suffixes.

For example:

* Builder methods are just a noun. It is `FooBuilder::bar(value)` not `FooBuilder::with_bar(value)`.
