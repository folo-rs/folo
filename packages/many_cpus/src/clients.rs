//! Establishes a client-server pattern whereby logic that uses the hardware tracker can
//! be replaced with a mock, breaking any hard dependencies for testing purposes.

mod hw_tracker_client;
mod hw_tracker_facade;

pub(crate) use hw_tracker_client::*;
pub(crate) use hw_tracker_facade::*;
