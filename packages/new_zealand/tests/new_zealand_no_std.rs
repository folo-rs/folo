//! Exercises macro expansion without importing the standard library into the consumer.

#![no_std]

use core::num::NonZero;

use new_zealand::nz;

#[test]
fn macro_works_without_std() {
    // Representative unsigned and signed values cover const and expression use.
    const UNSIGNED: NonZero<u16> = nz!(56_403);
    const SIGNED: NonZero<i32> = nz!(-42);
    let expression: NonZero<usize> = nz!(3 * 8);

    assert_eq!(UNSIGNED.get(), 56_403);
    assert_eq!(SIGNED.get(), -42);
    assert_eq!(expression.get(), 24);
}

::testing::set_allocator!();
