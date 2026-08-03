# cbh_engines - Implementation

`cbh_engines` implements benchmark-engine integration for the behavior described by the
[`cargo-bench-history` design](../../cargo-bench-history/docs/DESIGN.md). It owns injected engine
environment values, output discovery, schema-specific parsing, and conversion into the shared
`cbh_model` representation.

Each engine adapter keeps deserialization and model mapping local to the engine's schema.
Committed fixtures and producer-consumer schema round trips protect those boundaries. Parsers are
pure; filesystem output discovery is isolated behind its own port.

Each parser keeps its established public aggregate return type. Private semantic condition types
distinguish malformed documents and unsupported schema versions and retain `serde_json` errors as
sources. Downstream code observes only the aggregate and parser behavior, while defining-crate
unit tests verify exact mappings and payloads.
