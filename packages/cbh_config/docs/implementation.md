# cbh_config implementation

`cbh_config` supports the configuration behavior specified by the
[`cargo-bench-history` design](../../cargo-bench-history/docs/DESIGN.md). Its place in the
application is defined by the
[`cargo-bench-history` implementation guide](../../cargo-bench-history/docs/implementation.md)
and the workspace rules for [implementation documentation](../../../docs/implementation.md).

The crate owns the shared configuration model, configuration-file loading, and resolution of
command selections and ambient values into concrete paths. Parsing and loading remain separate
from input resolution so pure resolution functions receive environment values explicitly instead
of reading process-global state.

The optional ignore section carries `cbh_model::BenchmarkIdPrefix` values, reusing
their validation and literal matching contract rather than introducing a pattern language.
The section rejects unknown fields. Its default is no exclusions; interpretation belongs
only to analysis orchestration, not configuration loading or collection.

Configuration acquisition passes its read result to synchronous parsing and read-policy logic.
Unit tests cover that policy, including private error context, with in-memory read results;
Cargo integration tests cover loading real files.
The native read forwarder has a narrow mutation exclusion: detecting its replacement requires
real filesystem acquisition. Parsing, optional-file policy and error propagation remain mutation
targets in the synchronous interpreter.

Public configuration operations return one aggregate. Read, parse, and selection conditions
remain private, each retaining the context and lower-level cause owned by its responsibility. The
boundary follows the workspace [error-handling guide](../../../docs/error-handling.md).
