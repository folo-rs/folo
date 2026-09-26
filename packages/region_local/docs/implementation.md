# Region-local implementation

The [regional sharing design](design.md) is implemented by a linked family that owns shared
regional state and creates a local handle for each thread. Hardware topology determines which
regional state a handle accesses. A pinned thread can retain its regional state directly;
an unpinned thread resolves its current region when accessing a value.

Public constructors and static-variable macros obtain the current system hardware. The existing
hardware-taking constructor lets in-process tests supply fake topology and pinning state without
querying the host. Static extension tests initialize the underlying linked static wrapper through
that constructor, exercising publication within a region and isolation between regions. Cargo
integration tests exercise real constructors, the region-local macro, and dynamic linked wrappers.
Both test layers use the same regional storage implementation.
