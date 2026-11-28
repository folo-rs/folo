//! Contracts for selecting task destinations and owning policy-created workers.
//!
//! A custom policy uses two values with different lifetimes. Implement [`WorkerPolicy`] to
//! configure the policy before pool construction. [`PoolBuilder::worker_policy`][crate::PoolBuilder::worker_policy]
//! consumes that value and calls [`WorkerPolicy::bind`], which returns a [`BoundPolicy`] shared by
//! every scheduler attached to the completed pool.
//!
//! The binding phase creates [`WorkerGroup`] destinations through [`WorkerGroups`]. A group owns the
//! shared queues for its tasks, while [`WorkerGroupId`] is the stable value returned by
//! [`BoundPolicy::select_group`] on the hot path. Reserving a [`Worker`] is separate from routing:
//! the policy owns the operating-system thread, must run the reservation in that thread and may
//! drop an unrun reservation to cancel a binding-time readiness target.

mod bind_context;
mod bind_error;
mod policy;

pub use bind_context::BindContext;
pub use bind_error::BindError;
pub(crate) use policy::WorkerPolicyBinding;
pub use policy::{BoundPolicy, WorkerPolicy};

pub use crate::worker::Worker;
pub use crate::worker_group::WorkerGroup;
pub use crate::worker_groups::{WorkerGroupId, WorkerGroups};
