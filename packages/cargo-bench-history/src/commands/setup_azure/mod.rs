//! Standalone Azure bundle export and invocation, without benchmark dependencies.

mod bundle;
mod errors;
mod execute;
mod ports;

pub(crate) use execute::execute;

#[cfg(test)]
mod tests;
