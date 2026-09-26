//! Native batch execution for manifest-linked publication.

pub use batch::{Executor, Outcome, execute_items};
pub use github::Github;
pub use model::{Asset, Binary};
pub use publisher::BinaryPublisher;

mod batch;
mod github;
pub(crate) mod model;
pub mod publish;
mod publisher;
