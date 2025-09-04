---
mode: agent
---

The 9 pool types provided by the infinity_pool package are:

Blind Pools (3 types)
* RawBlindPool - Raw, unmanaged blind pool
* BlindPool - Managed blind pool (thread-safe)
* LocalBlindPool - Local blind pool (single-threaded)

Opaque Pools (3 types)
* RawOpaquePool - Raw, unmanaged opaque pool
* OpaquePool - Managed opaque pool (thread-safe)
* LocalOpaquePool - Local opaque pool (single-threaded)

Pinned Pools (3 types)
* RawPinnedPool<T> - Raw, unmanaged pinned pool for type T
* PinnedPool<T> - Managed pinned pool for type T (thread-safe)
* LocalPinnedPool<T> - Local pinned pool for type T (single-threaded)

Each category provides three variants:
* Raw: Manual memory management, no automatic cleanup
* Managed: Thread-safe with automatic cleanup
* Local: Single-threaded with automatic cleanup

The pools differ in their type safety approach:
* Blind: Type-erased, stores any type as raw bytes
* Opaque: Type-erased, stores types with matching layout
* Pinned: Fully typed, maintaining type information at compile time

Your task is to enhance the infinity_pool package:

1. Compare the unit test suites of each pool. Identify gaps in testing - some functionality
   that is only tested for some of the pools but is present in more pools, or lacking entirely.
   * Consider documented thread-safety behaviors.
   * Consider implemented traits.
   * Consider public API surface.
   * Consider which types of handles are used to reference pooled objects.
   * Consider iteration capabilities and designs.
   * Consider whether pools wrap another pool.
   * Consider whether pools expose strongly typed APIs or type-agnostic APIs.
   * Consider usage with Unpin types and with !Unpin types.
   * Consider usage with Send types and with !Send types.
   * Consider usage (or attempted usage) with zero-sized types.
   * Consider usage of builders.
   * Consider usage of pool capacity management functions.
   * Consider partial initialization of objects.
   * Consider conversion between exclusive and shared handles.
   * Consider usage of pools with different types or types of different layouts (where allowed).
   * Consider simultaneous access from multiple threads (where allowed).
   * Consider panic safety of executed closures.
   * Consider documented panic conditions in API documentation.
   * Consider behavior on pool drop, handle drop, and relative timing of each.
   * Consider pool growth, reservation and shrinking patterns.
   * Consider mixing up pool handles from different pools (where possible).
2. Create a list of the gaps between the pools.
3. Create a list of gaps that exist in all pools - it is possible some functionality
   is not tested in any of them or is not easily comparable between the pools.
4. Analyze whether the gaps are justified due to the differences in the pools' designs
   and feature sets or whether they are oversights.
5. Address any oversights by adding new unit tests and static assertions or
   by modifying existing ones if they need to be improved or enhanced.

Added unit tests should maintain parity between the .rs files of the different pool types:

* Use the same name for tests for the same functionality.
* Use equivalent assertions and test contents (as much as possible).
* Keep the tests in each file in the same order.

Consider not only the pools but also associated types like builders, iterators and handles. They
should all be thoroughly tested, achieving full coverage of the API surface - each function and
each branch in every function should be visited by at least one test.

Keep the tests concise and avoid testing different variants of the same thing unless justified.
Do not add unnecessary comments that restate what the code is doing. We can all read.

Test the documented behavior, not implementation details, unless explicitly instructed or
documented. If there is a gap in API documentation and documented behavior is ambiguous,
make a note of it for the user to clarify.