//! Temporary executable wiring for the bootstrap publisher's binary commands.

pub use run::run;

mod run;

#[cfg(test)]
::testing::set_allocator!();
