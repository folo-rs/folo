use std::{fmt::Debug, hash::Hash};

/// Bindings for FFI calls into external libraries (either provided by operating system or not).
///
/// All PAL FFI calls must go through this trait, enabling them to be mocked.
pub(super) trait Bindings:
    Clone + Copy + Debug + Eq + Ord + Hash + PartialEq + PartialOrd + Send + Sync + 'static
{
}

#[derive(Copy, Clone, Debug, Default, Eq, Ord, Hash, PartialEq, PartialOrd)]
pub(super) struct BindingsImpl;

impl Bindings for BindingsImpl {}
