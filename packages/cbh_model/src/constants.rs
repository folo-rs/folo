//! Shared constants for the pure core: the storage layout version and the fixed
//! object-partition segment used by the storage key layout.

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
