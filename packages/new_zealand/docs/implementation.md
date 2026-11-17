# Macro implementation

The library exports a declarative macro from a `#![no_std]` crate to implement the
[construction contract](design.md). An inline const block makes evaluation happen
at compile time even when the caller uses the macro in a runtime expression.

The expansion uses an absolute `::core::num::NonZero` path because exported macros
resolve that path in the consumer's crate. This keeps construction independent of
the consumer's imports and does not require the standard library.

The integration test compiles macro invocations in a `#![no_std]` consumer without
importing `std`, covering signed and unsigned constants and expression usage.
The test harness and development-only allocator remain outside the library's
runtime dependencies.
