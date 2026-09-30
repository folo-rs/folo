# cbh_engines implementation

`cbh_engines` implements the engine compatibility behavior specified by the
[`cargo-bench-history` design](../../cargo-bench-history/docs/DESIGN.md). Its place in the
application is defined by the
[`cargo-bench-history` implementation guide](../../cargo-bench-history/docs/implementation.md)
and the workspace rules for [implementation documentation](../../../docs/implementation.md).

The crate owns translation from each supported engine's artifacts into `cbh_model`, together with
the environment and harvesting boundaries needed to locate those artifacts. Each adapter owns its
external schema and mapping logic. Parsing stays pure; process execution remains owned by
`cbh_git`, and persistence remains owned by `cbh_storage`.

Each parser exposes an operation-level aggregate while concrete document and schema conditions
remain private to the adapter that understands them. Sources are preserved according to the
workspace [error-handling guide](../../../docs/error-handling.md). Live producer-consumer round
trips detect schema drift for the in-workspace `alloc_tracker` and `all_the_time` engines.
Committed Criterion and Callgrind fixtures regression-test their recorded external schemas; those
fixtures must be regenerated or the schemas otherwise validated when the external producers are
upgraded.

Orchestration tests share minimal synthetic engine documents through the test-support
module. These exercise the real adapters without repeatedly processing unrelated producer
metadata. The committed external-output fixtures and live producer round trips retain
schema-compatibility coverage at the adapter boundary.

Harvesting separates filesystem observations from engine traversal and selection. A private port
supplies directory cursors, entry type flags, modification times and contents. Acquisition remains
lazy and uses native entry metadata, preserving error order and link handling without reading
irrelevant files. The same collectors run against Tokio and in-process fixtures; engine dispatch,
recursion, pairing, filtering, freshness and missing-versus-operational-error decisions remain
mutation targets. Only individual native acquisition forwarders and their composition entry point
have integration-only mutation exclusions.

Real filesystem harvesting and live producer round trips run as Cargo integration tests because
they acquire real files or processor-clock readings. Harvesting fixtures assign their modification
times explicitly rather than depending on the wall clock. In-process harvesting fixtures likewise
supply fixed timestamps and controlled read failures, alongside pure parsing, freshness arithmetic
and diagnostic formatting tests.
