# Region-cached implementation

The [regional caching design](design.md) is implemented by a linked family with an authoritative
shared value and regional cached copies. Publication invalidates those copies; readers initialize
their region from the shared value while checking generation consistency. Pinned handles can
retain their regional state, while unpinned handles resolve the current region on access.

Public constructors and static-variable macros obtain the current system hardware. In-process
tests use the existing hardware-taking constructor with fake topology and pinning state. Cargo
integration tests exercise real constructors, macro expansion, and dynamic linked wrappers.
The acquisition boundary does not change the cache or publication implementation between tests
and production.
