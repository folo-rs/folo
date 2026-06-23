//! Gungraun/Callgrind cache event-kind names, shared by the shell's Callgrind
//! output parser and the analysis layer's direction logic.
//!
//! Only L1 hits are higher-is-better: an access served by L1 is the cheap outcome.
//! Last-level and RAM hits are the expensive tiers — an access that falls through
//! to them is a cache miss escalating to slower memory, so more of them is worse.

/// Gungraun event-kind name for an access served by the L1 cache (cheap).
pub const L1_HITS_EVENT: &str = "L1hits";
/// Gungraun event-kind name for an access served by the last-level cache.
pub const LL_HITS_EVENT: &str = "LLhits";
/// Gungraun event-kind name for an access served by main memory (expensive).
pub const RAM_HITS_EVENT: &str = "RamHits";
