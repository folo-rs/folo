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

Error types are built with `ohno`. A condition leaf represents one semantic
failure. Leaves are private implementation details; the public boundary depends
on whether the package is a library or an application.

Place an error type next to the API that produces it when it belongs to exactly
one module, as `cbh_engines` does with `CallgrindParseError` in
`bench/callgrind.rs`. Collect a family into `src/errors.rs` or `src/error.rs` once
it spans several modules or grows past a handful, as in `cbh_analyze`,
`cbh_config`, `cbh_storage`, and the three CLI applications.

### Library boundaries

A library defines one private or `pub(crate)` leaf per distinct failure condition.
Its stable public API returns an aggregate for each behavioral family that can
fail. The aggregate is public; its leaves, fields, constructors, and exact
taxonomy are not.

```rust
#[ohno::error]
pub(crate) struct NotAFileError;

#[ohno::error]
pub(crate) struct ManifestParseError;

#[ohno::error]
#[no_constructors]
#[from(NotAFileError, ManifestParseError)]
pub struct ManifestError;
```

The aggregate is transparent by default. Omitting `#[display]` makes it render its
source verbatim with no `caused by:` line, while `#[no_constructors]` prevents a
sourceless aggregate that would render the literal type name.

An external production branch may distinguish only decisions the library
explicitly supports. Add a private aggregate-owned decision state and a narrow
public query such as `is_not_found()` only for a documented production caller.
Explicit leaf-to-aggregate conversions populate that state. Do not publish a
closed leaf-shaped `kind` enum or exact condition query merely for tests.

Within the defining crate, implementation code and unit tests may use
`find_source::<PrivateLeaf>()` (with `use ohno::ErrorExt;`; on `AppError` it is
inherent). Outside the crate, callers may locate the public aggregate in a source
chain and use its supported narrow queries, but cannot name or inspect its leaves.

### Application boundaries

Applications use private condition leaves and return `ohno::AppError` from
application-facing boundaries (enable the `app-err` feature). They do not add a
package-specific public aggregate, export application leaves, or re-export error
aggregates and leaves from component libraries. A component library's public
aggregate converts into `AppError` without flattening its source chain.

Same-crate application logic may inspect private leaves when a real control-flow
decision requires it. That does not make the condition part of the application's
public API.

### Error tests do not define visibility

Exact leaf and field assertions belong in the defining module's unit tests.
Integration tests and downstream packages validate the public return type,
observable behavior, side effects, supported aggregate queries, or a deliberately
unsupported `private-test-util` hook. Test placement never justifies making a
leaf, constructor, field, accessor, or aggregate internal public.

Further rules:

* Generated `new`/`caused_by` are always `pub(crate)` regardless of the type's
  visibility and cannot be overloaded. A genuine public API requirement for caller
  construction needs `#[no_constructors]` and a hand-written constructor; integration
  tests are not such a requirement. Under `#[derive(ohno::Error)]`, manual construction
  also requires an `#[error] core: OhnoCore` field. The `#[ohno::error]` attribute form
  injects that field, so a hand-written constructor initializes `ohno_core`.
* `#[display]` arguments are implicitly rewritten as `&self.<expr>`, so write
  `path.display()`, never `self.path.display()`. Literals and constants are not
  accepted. On a tuple struct, `{0}` fails — write `#[display("{}", 0)]`.
* No `Clone` is generated; derive it explicitly when needed and pin it with
  `static_assertions`. `PartialEq`, `Eq` and `Copy` cannot be recovered at all,
  because the type carries a `Backtrace` and an `Arc`.
* Map a foreign failure into a private semantic leaf at the call site before
  converting it to a library aggregate or `AppError`. A direct aggregate
  `From<ForeignError>` is safe only when the source draws no distinction the
  destination also draws. For example, `io::ErrorKind::NotFound` means object absence
  in storage `get`/`delete`, an empty subtree in `list`, and an operation failure in
  `put`; only the call site can select the correct leaf and aggregate decision.
* Never fold another error's `to_string()` or `message()` into a `String` field.
  Attach it as a source with `caused_by` instead. A folded string bakes in that
  error's backtrace, which then reappears wherever the field is rendered — including
  in summaries printed on success paths.
* Do not prefix a message with its category (`"I/O error: "`, `"configuration
  error: "`). The type already names the condition, and the wrapper it travels
  through is transparent, so the prefix only adds noise.
* Prefer a type that names the operation over one that just wraps `io::Error`: a
  bare `io::Error` says what went wrong but never what was being attempted.
* Add a field accessor only when a caller outside the defining module reads it.
  In-module tests read private fields directly; a sibling module means widening the
  field or accessor to `pub(crate)`, not adding a public accessor.

Each error carries its own backtrace, so a wrapper chain prints one backtrace block
per level under `RUST_BACKTRACE=1`. Never assert on a full `to_string()`. See the
unwind-safety chapter for the manual `UnwindSafe`/`RefUnwindSafe` impls these types
require.
