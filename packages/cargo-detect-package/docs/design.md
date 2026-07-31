# cargo-detect-package - Design

`cargo-detect-package` derives the Cargo package scope for a target path and applies that
scope to another command. It exists so editor and automation entry points can select the same
package a developer would choose without duplicating Cargo-workspace discovery rules.

## Scope selection

The current directory establishes the active workspace, while the target path identifies the
candidate package. Both must resolve within the same workspace before another command is
started. This prevents a path from silently causing work in an unrelated checkout.

A target inside a non-root package selects that package. The workspace-root package is excluded
from package selection, so targets it owns follow the same explicit policy as targets outside
every package: use workspace scope, succeed without running the command, or fail. The selected
scope is conveyed either through Cargo package arguments or an environment variable, allowing
Cargo commands and other automation tools to share one detection model.

Workspace discovery, package selection, and command execution are distinct boundaries.
External execution happens only after scope validation succeeds, so a discovery failure
cannot launch a command with guessed or partial scope.

## Diagnostics and error boundary

Successful scope decisions are reported separately from failures. The CLI reports failures on
stderr and returns a failure status; a child command's status remains the outcome of that child
rather than being reclassified as a discovery failure.

Operational failures are distinct typed conditions with their original filesystem, manifest,
or process errors retained as sources. The application boundary carries them through
`ohno::AppError`, which preserves causal diagnostics and allows callers to identify a specific
condition without imposing a closed error taxonomy. Context is added at the boundary that
knows it, while source messages are not flattened into strings or redundantly prefixed.
