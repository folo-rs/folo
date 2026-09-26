//! Native batch execution for manifest-linked publication.

pub use batch::{Executor, Outcome, execute_items};
pub use model::{Asset, Binary};
pub use native::{Github, Native};

mod archive;
mod batch;
pub(crate) mod command;
pub(crate) mod model;
pub(crate) mod native;
pub(crate) mod publish;
mod source;
