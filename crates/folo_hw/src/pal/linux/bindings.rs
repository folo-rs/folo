use std::fmt::Debug;

/// Bindings for FFI calls into external libraries (either provided by operating system or not).
///
/// All PAL FFI calls must go through this trait, enabling them to be mocked.
pub(super) trait Bindings: Debug + Send + Sync + 'static {}

#[derive(Debug, Default)]
pub(super) struct BindingsImpl;

impl Bindings for BindingsImpl {}
