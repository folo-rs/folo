//! Coordinates frozen binary releases and projects their delivery outcomes.
//!
//! A controller selects immutable tag sources into per-target batches. `BinaryPublisher`
//! composes native-owned source worktrees, processes and artifacts with publication-owned
//! asset discovery and delivery. The manifest-linked command and bootstrap adapter share
//! this sequence. See the publication and native implementation guides for ownership.

pub use batch::{Executor, Outcome, execute_items};
pub use github::Github;
pub use model::{Asset, Binary};
pub use publisher::BinaryPublisher;

mod batch;
mod github;
pub(crate) mod model;
pub mod publish;
mod publisher;
