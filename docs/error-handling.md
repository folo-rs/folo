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
