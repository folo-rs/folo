# Error handling

This chapter covers the rules around `Result`, `Option`, panics, arithmetic
overflow, and `drop()`-time invariant checks in production code. Test-side rules
for asserting that errors and panics happen are in the testing chapter.

## Use of unwrap() and expect()

Use `unwrap()` in test code and only in test code. Do not use `expect()` in test
code.

You may use `expect()` in non-test code but only if there is a reason to believe
that the expectation will never fail. That is, we do not use `expect()` as an
assertion — we use it to cut off unreachable code paths. The message inside
`expect()` should explain why we can be certain that code path is unreachable; it
is not an error message saying what went wrong!

State clearly in the `expect()` message why the expectation is guaranteed to
hold. Do not use words like "should" — if it only "should" hold, then you have
failed to establish a guarantee!

Using `assert!()` or other panic-inducing macros in non-test code is fine as long
as it is documented in API documentation (in a `# Panics` section). Treat the
`assert!()` message the same as the message for `expect!()` — it should justify
why we expect the assertion to hold. If we do not expect the assertion to hold
but are merely fulfilling an API contract to panic, no assert message is to be
used. Similarly, do not use assertion messages in tests.

## Checked arithmetic

Unless there is a specific reason to use saturating/wrapping arithmetic, use
checked arithmetic (`.checked_add()` and similar) and handle the error case. Do
not use regular unchecked arithmetic (`+`, `-`, `*`, `/`, `%`) as it can overflow
and panic.

It is fine to `.expect()` success if there is some reason to believe overflow can
never happen, e.g. because it is guarded by an assertion or because it would
require some data structure to exceed the size of virtual memory. If very
confident that an overflow can never occur, it is fine to use wrapping arithmetic
via explicit `.wrapping_add()` methods.

This only applies to non-test code — in tests and benchmarks, it is fine to use
whatever arithmetic is most convenient.

## Panic in drop()

It is OK to verify that type invariants still hold in `drop()` and panic if some
have been violated, e.g. when an item is not in a valid state to be dropped.
However, all such assertions must be guarded with a `thread::panicking()` check
to ensure that the panic does not occur when already unwinding for another panic
— we do not want to double-panic as that mangles the errors.

## Non-zero integers

Whenever a numeric value must be non-zero, prefer `NonZero<usize>` over `usize`,
in both private APIs/logic and public APIs. Prefer `NonZero<usize>` over
`NonZeroUsize`.

## Defining error types

Error types are built with `ohno`. Applications return `ohno::AppError` (enable the
`app-err` feature) at their public entry points. Libraries expose a public aggregate
for each behavioral family of operations.

Place an error type next to the API that produces it when it belongs to exactly one
module — `cbh_config`'s `ConfigError` lives in `config.rs`, `cbh_engines`'
`CallgrindParseError` in `bench/callgrind.rs`. Collect them into a dedicated
`src/errors.rs` (or `src/error.rs`, matching whichever name the package already
uses) once they span several modules or grow past a handful, as in `cbh_analyze`,
`cbh_storage`, and the three CLI binaries.

`ohno` rejects enums, so define **one private leaf type per distinct failure
condition**. A library groups those leaves under the public aggregate returned by
the relevant API. The aggregate is transparent by default:

```rust
#[ohno::error]
#[no_constructors]
#[from(NotAFileError, ManifestParseError)]
pub struct ManifestError;
```

Application-owned leaves remain private and convert directly into `AppError`.
Application integration tests assert the `AppError` boundary and observable
behavior; exact leaf and field mappings belong in same-crate unit tests. Test
placement never justifies making a leaf, constructor, field, or accessor public.

Omitting `#[display]` on a type that carries a source makes it render the source's
message verbatim with no `caused by:` line, so wrapping adds nothing a user sees.
`#[no_constructors]` is what stops such a wrapper from being constructed without a
source, which would render the literal type name.

Same-crate implementation and unit-test code can discriminate private leaves with
`find_source::<T>()` (needs `use ohno::ErrorExt;`; on `AppError` it is inherent).
When an external production caller has a documented need to branch, the public
aggregate owns the smallest decision state that supports that behavior and exposes
narrow query methods. Populate the decision state through explicit leaf-to-aggregate
conversions. Do not expose the private leaf or mirror the complete private taxonomy
in a public kind merely because tests could inspect it.

Further rules:

* Generated `new`/`caused_by` are always `pub(crate)` regardless of the type's
  visibility, and cannot be overloaded — a hand-written constructor beside them is
  a duplicate definition. Public aggregates normally use `#[no_constructors]` and
  are created only by conversions from private leaves. Under
  `#[derive(ohno::Error)]`, a hand-written conversion initializes the declared
  `#[error] core: OhnoCore` field. The `#[ohno::error]` attribute form injects that
  field itself.
* `#[display]` arguments are implicitly rewritten as `&self.<expr>`, so write
  `path.display()`, never `self.path.display()`. Literals and constants are not
  accepted. On a tuple struct, `{0}` fails — write `#[display("{}", 0)]`.
* No `Clone` is generated; derive it explicitly when needed and pin it with
  `static_assertions`. `PartialEq`, `Eq` and `Copy` cannot be recovered at all,
  because the type carries a `Backtrace` and an `Arc`.
* Foreign failures enter through a private semantic leaf that names the operation
  or condition and owns the foreign error as its source. A direct `From` for a
  foreign error is safe only when the source draws no distinction the destination
  also draws. `io::Error` carries its own `kind()`, and a storage caller may treat
  `NotFound` as a cache miss, a skipped directory, or an operation failure depending
  on the call site. Without a blanket conversion, `?` forces the producer to choose
  the correct semantic leaf.
* Never fold another error's `to_string()` or `message()` into a `String` field.
  Attach it as a source with `caused_by` instead. A folded string bakes in that
  error's backtrace, which then reappears wherever the field is rendered — including
  in summaries printed on success paths.
* Do not prefix a message with its category (`"I/O error: "`, `"configuration
  error: "`). The type already names the condition, and the wrapper it travels
  through is transparent, so the prefix only adds noise.
* Prefer a type that names the operation over one that just wraps `io::Error`: a
  bare `io::Error` says what went wrong but never what was being attempted.
* Leaf fields and their accessors remain private unless same-crate production code
  needs wider access. In-module tests read private fields directly; a sibling module
  means widening the field or accessor to `pub(crate)`, not adding public API.

Each error carries its own backtrace, so a wrapper chain prints one backtrace block
per level under `RUST_BACKTRACE=1`. Never assert on a full `to_string()`. See the
unwind-safety chapter for the manual `UnwindSafe`/`RefUnwindSafe` impls these types
require.
