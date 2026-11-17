# Non-zero integer construction

`new_zealand` provides `nz!` for constructing non-zero integers from compile-time
expressions. Callers can use signed or unsigned integer types and rely on type
inference to select the result's `NonZero<T>` type. Expressions must be constant
and evaluate to a non-zero value.

The crate works in both standard-library and `#![no_std]` applications without an
allocator. Construction requires no runtime initialization or allocation.
