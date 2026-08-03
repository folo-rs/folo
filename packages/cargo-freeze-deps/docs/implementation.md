# cargo-freeze-deps - Implementation

The command reads the selected manifest, parses it with `toml_edit`, walks each supported
dependency table, and writes the rendered document to the selected output path. `toml_edit`
preserves comments and document layout while changed version values use its standard rendering.

Version requirements are parsed with `semver`. A requirement is frozen only when one comparator
provides a concrete matching version; missing minor or patch components are zero-filled.
Unsupported constraint shapes remain unchanged.

Distinct failure conditions use `ohno` error types and flow through `AppError`. File and parse
errors carry the relevant path, and wrapped source errors remain discoverable through the source
chain.
