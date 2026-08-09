# cargo-detect-package implementation

User-visible scope selection and command behavior belong in the package
[design](design.md). This guide follows the workspace rules for
[implementation documentation](../../../docs/implementation.md).

The application is organized around workspace discovery, manifest parsing, package detection, and
child-process execution. A composition layer coordinates those responsibilities. Filesystem access
passes through the package's platform abstraction so discovery logic does not depend directly on
the host filesystem.

Operational conditions are private to the responsibility that can add their context and flow into
the application's `ohno::AppError` boundary. Lower-level filesystem, manifest, and process causes
remain attached. The package exports neither those conditions nor a package-specific aggregate, in
accordance with the workspace [error-handling guide](../../../docs/error-handling.md).
