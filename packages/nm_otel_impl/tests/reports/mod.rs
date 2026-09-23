//! SDK-backed export assertions using supplied reports instead of real event collection.

#![cfg(not(miri))]

mod mapping;
mod publisher;
