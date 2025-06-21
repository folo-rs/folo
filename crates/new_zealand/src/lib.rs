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
