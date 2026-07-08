#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Utilizing for working with non-zero integers.
//!
//! Currently this implements a shorthand macro for creating non-zero integers
//! from expressions at compile time:
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
//! foo(nz!(3 * 8));
//! ```

mod nz;
