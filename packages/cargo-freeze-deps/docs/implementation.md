# cargo-freeze-deps implementation

The transformation contract belongs in the package [design](design.md). This guide follows the
workspace rules for [implementation documentation](../../../docs/implementation.md).

Manifest I/O, structural TOML editing, dependency-table traversal, and version-requirement
interpretation are separate responsibilities composed by the application entry point. Structural
editing is isolated behind `toml_edit`, while requirement interpretation is isolated behind
`semver`.

Private conditions add context at the responsibility that owns it and flow into the application's
`ohno::AppError` boundary. Parser, semantic-version, and filesystem causes remain attached without
becoming application API, following the workspace
[error-handling guide](../../../docs/error-handling.md).
