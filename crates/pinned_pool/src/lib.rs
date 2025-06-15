//! An object pool that guarantees pinning of its items

#![allow(dead_code, unused_imports, reason = "temporary")]

mod builder;
mod drop_policy;
mod pinned_pool;
mod pinned_slab;

pub(crate) use builder::*;
pub(crate) use drop_policy::*;
pub use pinned_pool::*;
pub(crate) use pinned_slab::*;
