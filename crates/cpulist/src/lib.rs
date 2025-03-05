#![doc = include_str!("../README.md")]

mod emit;
mod error;
mod parse;

pub use emit::*;
pub use error::*;
pub use parse::*;

pub(crate) type Item = u32;
