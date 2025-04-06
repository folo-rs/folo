//! Utilities for internal use in Folo crates; no stable API surface

/// A macro to create a `NonZero` constant from a literal value.
#[macro_export]
macro_rules! nz {
    ($x:literal) => {
        const { ::std::num::NonZero::new($x).expect("literal must have non-zero value") }
    };
}
