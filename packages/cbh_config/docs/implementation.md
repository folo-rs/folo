# cbh_config - Implementation

`cbh_config` implements the configuration and input-resolution behavior described by the
[`cargo-bench-history` design](../../cargo-bench-history/docs/DESIGN.md). The application owns the
user-visible contract; this crate owns TOML parsing, optional-file loading, the starter template,
and resolution of command selections and environment values into paths and project identity.

Parsing and path resolution remain pure where possible. Environment and filesystem reads are kept
at narrow I/O edges, while callers pass their values into deterministic resolvers. This avoids
process-global state in unit tests.

Public configuration operations return one transparent `ConfigError` aggregate. Private semantic
conditions distinguish file reads, TOML/schema parsing, and selection options that require an
environment value. Foreign I/O and TOML errors remain sources of those conditions; the aggregate
does not expose constructors or a public category taxonomy.
