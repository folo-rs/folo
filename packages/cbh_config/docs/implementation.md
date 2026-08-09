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

Public configuration operations return one aggregate. Read, parse, and selection conditions
remain private, each retaining the context and lower-level cause owned by its responsibility. The
boundary follows the workspace [error-handling guide](../../../docs/error-handling.md).
