//! Repository-specific relevance check for paired action releases.

mod check;
mod run;

pub use run::run;

#[cfg(test)]
::testing::set_allocator!();
