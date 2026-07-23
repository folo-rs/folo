//! String-newtype identifiers for two opaque text values that would otherwise be
//! indistinguishable `String`s: the [`TargetTriple`] a run was measured on and the
//! [`MachineKey`] its history is partitioned by. Each is sanitized when it becomes
//! a path segment in a storage key, but also appears in raw form elsewhere — most
//! notably [`TargetTriple`], which holds the value `rustc` reported in a
//! [`ToolchainInfo`](crate::ToolchainInfo).
//!
//! As storage-key segments the two sit side by side, so as bare strings they are
//! trivially transposed where they are constructed or passed together — most
//! visibly in [`DiscriminantSet::new`](crate::DiscriminantSet::new) and the
//! flattened JSON reports. Distinct types make that transposition a compile error
//! while preserving the stored wire format (`#[serde(transparent)]`) and the
//! string ordering a [`DiscriminantSet`](crate::DiscriminantSet) sorts by.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Defines a transparent `String` newtype with the shared accessor, conversion,
/// display, and string-comparison conveniences the two identifiers need.
macro_rules! string_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// The identifier as a string slice.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.0 == *other
            }
        }
    };
}

string_newtype! {
    /// The target triple a run was measured on (for example,
    /// `x86_64-unknown-linux-gnu`).
    ///
    /// A `DiscriminantSet` holds the sanitized, path-segment form; a
    /// [`ToolchainInfo`](crate::ToolchainInfo) holds the raw value `rustc`
    /// reported. Both are the same type — the sanitization is a step, not a
    /// distinct kind of triple.
    TargetTriple
}

string_newtype! {
    /// The machine key a series is partitioned by: a stable hardware fingerprint
    /// or an explicit `--machine-key` override.
    ///
    /// Every engine is machine-keyed, so this segment always participates in a
    /// storage key alongside the [`TargetTriple`]; the distinct types keep the two
    /// from being transposed.
    MachineKey
}
