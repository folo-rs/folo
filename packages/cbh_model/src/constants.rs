//! Shared constants for the pure core: the storage layout version and the
//! Gungraun/Callgrind cache event-kind names that the shell's Callgrind output
//! parser maps to metric kinds.
//!
//! The cache event-kind strings are the raw values Gungraun emits. The shell's
//! Callgrind adapter matches against them to translate each event into the
//! corresponding [`MetricKind`](crate::MetricKind); the per-kind comparison
//! polarity then lives on the kind itself (only L1 hits are higher-is-better — an
//! access served by L1 is the cheap outcome, while last-level and RAM hits are the
//! expensive miss-escalation tiers).

/// Version segment that prefixes every storage object key.
///
/// It is the first segment of a partition prefix
/// (`{STORAGE_VERSION}/{project}/{OBJECTS_SEGMENT}/{engine}/{triple}/{machine}`)
/// and the value that `parse_key` requires before it
/// accepts a key, so the writer and reader share a single source of truth for the
/// layout version.
pub const STORAGE_VERSION: &str = "v1";

/// The fixed segment, directly under a project, below which all benchmark **data**
/// objects live (`{STORAGE_VERSION}/{project}/{OBJECTS_SEGMENT}/…`).
///
/// It separates a project's data subtree from its per-project **metadata** siblings
/// — today only the cache-invalidation marker
/// (`{STORAGE_VERSION}/{project}/_cache-epoch`), with room for future kinds such as
/// indexes — so a data listing narrows to this prefix and can never enumerate a
/// marker, and a metadata object can never be mistaken for a data key.
pub const OBJECTS_SEGMENT: &str = "objects";

/// Gungraun event-kind name for an access served by the L1 cache (cheap).
pub const L1_HITS_EVENT: &str = "L1hits";
/// Gungraun event-kind name for an access served by the last-level cache.
pub const LL_HITS_EVENT: &str = "LLhits";
/// Gungraun event-kind name for an access served by main memory (expensive).
pub const RAM_HITS_EVENT: &str = "RamHits";
