//! Utilizing for working with non-zero integers.
//!
//! Currently this implements a shorthand macro for creating non-zero integers
//! from literals at compile time:
//!
//! ```
//! use std::num::NonZero;
//!
//! use new_zealand::nz;
//!
//! fn foo(x: NonZero<u32>) {
//!     println!("NonZero value: {x}");
//! }
//!
//! foo(nz!(42));
//! ```

/// A macro to create a `NonZero` constant from a literal value.
#[macro_export]
macro_rules! nz {
    ($x:literal) => {
        const { ::std::num::NonZero::new($x).expect("literal must have non-zero value") }
    };
}

#[cfg(test)]
mod tests {
    use std::num::NonZero;

    use crate::nz;

    #[test]
    fn test_basic_functionality() {
        // Test basic usage with different data types
        let u32_value: NonZero<u32> = nz!(42);
        assert_eq!(u32_value.get(), 42);

        let u64_value: NonZero<u64> = nz!(1_000_000);
        assert_eq!(u64_value.get(), 1_000_000);

        let usize_value: NonZero<usize> = nz!(12345);
        assert_eq!(usize_value.get(), 12345);
    }

    #[test]
    fn test_signed_integers() {
        // Test positive and negative values for signed types
        let positive: NonZero<i32> = nz!(42);
        assert_eq!(positive.get(), 42);

        let negative: NonZero<i32> = nz!(-42);
        assert_eq!(negative.get(), -42);

        let large_positive: NonZero<i64> = nz!(9_223_372_036_854_775_807);
        assert_eq!(large_positive.get(), 9_223_372_036_854_775_807);

        let large_negative: NonZero<i64> = nz!(-9_223_372_036_854_775_808);
        assert_eq!(large_negative.get(), -9_223_372_036_854_775_808);
    }

    #[test]
    fn test_extreme_values() {
        // Test minimum non-zero and maximum values
        let min_u8: NonZero<u8> = nz!(1);
        assert_eq!(min_u8.get(), 1);

        let max_u8: NonZero<u8> = nz!(255);
        assert_eq!(max_u8.get(), 255);

        let max_i8: NonZero<i8> = nz!(127);
        assert_eq!(max_i8.get(), 127);

        let min_i8: NonZero<i8> = nz!(-128);
        assert_eq!(min_i8.get(), -128);
    }

    #[test]
    fn test_const_evaluation() {
        // Test that the macro works in const contexts
        const U32_VALUE: NonZero<u32> = nz!(42);
        const I32_VALUE: NonZero<i32> = nz!(-42);
        const U64_VALUE: NonZero<u64> = nz!(1_000_000);

        assert_eq!(U32_VALUE.get(), 42);
        assert_eq!(I32_VALUE.get(), -42);
        assert_eq!(U64_VALUE.get(), 1_000_000);
    }

    #[test]
    fn test_function_parameter() {
        fn takes_nonzero_u32(x: NonZero<u32>) -> u32 {
            x.get() * 2
        }

        fn takes_nonzero_i32(x: NonZero<i32>) -> i32 {
            x.get() * 2
        }

        assert_eq!(takes_nonzero_u32(nz!(21)), 42);
        assert_eq!(takes_nonzero_i32(nz!(-21)), -42);
    }

    #[test]
    fn test_various_literal_formats() {
        // Test decimal literals
        let decimal: NonZero<u32> = nz!(123);
        assert_eq!(decimal.get(), 123);

        // Test hexadecimal literals
        let hex: NonZero<u32> = nz!(0xFF);
        assert_eq!(hex.get(), 255);

        // Test binary literals
        let binary: NonZero<u32> = nz!(0b1111_1111);
        assert_eq!(binary.get(), 255);

        // Test octal literals
        let octal: NonZero<u32> = nz!(0o377);
        assert_eq!(octal.get(), 255);
    }

    #[test]
    fn test_type_inference() {
        // Test that the macro works with type inference
        let inferred = nz!(42);
        let _: NonZero<u32> = inferred;
        assert_eq!(inferred.get(), 42);
    }
}
