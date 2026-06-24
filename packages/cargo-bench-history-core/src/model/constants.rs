//! Shared constants for the pure core: the Gungraun/Callgrind cache event-kind
//! names that the shell's Callgrind output parser maps to metric kinds.
//!
//! These are the raw event-kind strings Gungraun emits. The shell's Callgrind
//! adapter matches against them to translate each event into the corresponding
//! [`MetricKind`](crate::model::MetricKind); the per-kind comparison polarity then
//! lives on the kind itself (only L1 hits are higher-is-better — an access served
//! by L1 is the cheap outcome, while last-level and RAM hits are the expensive
//! miss-escalation tiers).

/// Gungraun event-kind name for an access served by the L1 cache (cheap).
pub const L1_HITS_EVENT: &str = "L1hits";
/// Gungraun event-kind name for an access served by the last-level cache.
pub const LL_HITS_EVENT: &str = "LLhits";
/// Gungraun event-kind name for an access served by main memory (expensive).
pub const RAM_HITS_EVENT: &str = "RamHits";
