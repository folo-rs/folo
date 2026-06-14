# Code style

Use the nightly toolchain to run `cargo fmt` because we rely on some of its formatting features.

## Line length and Clippy

We limit line length to 100, including API documentation and comments.

There are many Clippy rules defined in the workspace-level `Cargo.toml`.

Follow Clippy rules even in example code that is part of API documentation.
Note that Clippy does not actually check this code, so you have to remember to
apply any Clippy fixes autonomously - if you fix something in real code, see if
there are some (unreported) places in API documentation that also need the same fix.

## Language

Use proper English grammar, spelling and punctuation.

Titles are normal sentences, do not capitalize every word in a title.

Sentences (including in one line comments) end with punctuation:

* This is wrong: `// Create a pool for storing u64 values`
* This is correct: `// Create a pool for storing u64 values.`

Do not forget proper language in tests and examples (both standalone and inline examples).

Be professional. Do not use contractions. Use "do not" instead of "don't", for example.

## Whitespace

There should be one empty line between functions.

## Discarding values

Use `_ = expr;` to discard values, not `let _ = expr;`. The `let` keyword is
unnecessary and triggers `clippy::let_underscore_must_use`.

## Cloning into closures

When cloning a variable to move it into a closure, create a separate scope for the
clone and shadow the variable instead of polluting the parent scope with renamed variables.

This applies to all closure types: async blocks, thread spawns, and regular closures.

Bad:

```rust
let handle = mutex.clone();
spawn(async move { handle.something(); });
```

Good:

```rust
spawn({
    let mutex = mutex.clone();
    async move {
        mutex.something();
    }
});
```

The same pattern applies to thread spawning:

```rust
thread::spawn({
    let set = Arc::clone(&set);
    move || {
        set.do_work();
    }
});
```

## Variable shadowing for type conversions

When converting a variable to a different form (e.g. removing a `Pin` wrapper,
dereferencing a pointer to a reference, or re-pinning a reference), prefer
shadowing the original variable name instead of inventing a new abbreviated name.
This keeps the code readable and avoids cryptic short names like `aw`, `aw_ref`,
or `raw`.

Good:

```rust
let awaiter = unsafe { awaiter.get_unchecked_mut() };
let awaiter = ptr::from_mut(awaiter);
let awaiter = unsafe { &*awaiter };
```

Bad:

```rust
let aw = unsafe { awaiter.get_unchecked_mut() };
let raw = ptr::from_mut(aw);
let aw_ref = unsafe { &*raw };
```

Only use a different name when both forms are needed simultaneously in the same
scope.

## Comments must add value not re-state the code

Comments that merely restate what is obvious from the code must not exist.

Comments must add value by explaining why something is done, what it accomplishes, or how it works.

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

## Code files do not have chapters

Do not use section separator comments (e.g. `// --- Section ---` or `// ======= Title =======`)
to create visual "chapters" in code. Code organization should be clear from naming and structure.

## Design documentation

Document design elements, key decisions and architectural choices in inline
comments in the files to which they apply. Use regular `//` comments, not API
documentation comments.

Put package-level design documentation in `package-name/docs/design.md`. It is expected that
all major refactoring or redesign works are accompanied by design documentation which is kept
up to date when design changes are made.

If a package has multiple significant sub-components, create and maintain design documents for
each of them as `package-name/docs/component1.md` and similar, referenced from the `design.md`.

## Named constants

Avoid magic values in the code and use named constants instead. It does not matter
how many times the magic value is present, even one instance is enough to warrant
a named constant.

This includes zero. `0` is just as much a magic value as `1` or `42` when it
represents a domain concept (e.g. an initial state, a sentinel index, an empty
bitfield, the absence of flags). Define a named constant (`IDLE`, `EMPTY`, `NONE`,
`ROOT`, etc.) and use it instead. Numeric `0` only stays unadorned when it is
literally arithmetic (e.g. `for i in 0..n`, `x.saturating_sub(0)`,
`Vec::with_capacity(0)`).

If constants are only used in one function, put them at the top of that function.

The exception is example code — if a magic value is only used once in an example,
it is fine to leave it inline, as examples must prioritize being concise and readable.

## Keep names simple and unadorned

Avoid unnecessary and repetitive prefixes and suffixes.

For example:

* Builder methods are just a noun. It is `FooBuilder::bar(value)` not
  `FooBuilder::with_bar(value)`.

See [`docs/naming.md`](naming.md) for specific naming conventions.

## Type names

Do not hardcode type names in string literals (e.g. in `impl fmt::Debug`).
Instead use `type_name::<Self>()` or similar to refer to type names.

## Use imports and do not reference types via absolute paths

Types, functions and other items we reference in our code should be imported via `use` statements.

Unless there is a specific need to disambiguate between similarly named types,
do not use absolute paths for referencing items.

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

It is fine to rely on the Rust prelude — no need to explicitly import types from
the Rust prelude.

`use` statements go at the top of the file or module, not inside functions.

Do not `use super::` except in unit tests. Instead, use the full path through `use crate::`.

Exceptions:

* Refer to `std::alloc::System` by full path because the short form of this causes
  AI hallucinations.

## Import items by their public path

If an item is available via a private (e.g. `pub(crate)`) path and a public path,
use the public path.

Wrong:

```rust
use crate::session::AllocationTrackingSession;
```

Correct:

```rust
use crate::AllocationTrackingSession;
```

## Re-export defaults to wildcard

We prefer to re-export public types from private modules using a wildcard import.
There is no need to name the types separately unless we are intentionally filtering.

Good:

```rust
pub use events::*;
```

Bad:

```rust
pub use events::{Event, LocalEvent, EventPool, LocalEventPool};
```

## Lint suppressions

It is fine to suppress Clippy and compiler lints in the code if it is justified.
All suppressions must have a `reason` field to justify them.

Prefer `expect` over `allow` suppressions, except when applying a broadly-scoped
suppression that applies to a whole file or module using inner attributes, in
which case "allow" is preferred.

## Do not use `\n` in println!() statements

To emit an empty line to the terminal, use `println!();` instead of embedding `\n`
in another printed line.

## YAML formatting

Prefer not using quotes around strings, unless the string starts with special
characters.

Example of desired formatting:

```yaml
regular_field: just some text
special_field: ':::starts with special characters, needs quoting'
```
