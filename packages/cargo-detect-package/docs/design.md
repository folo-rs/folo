# cargo-detect-package - Design

`cargo-detect-package` derives the Cargo package scope for a target path and applies that
scope to another command. It exists so editor and automation entry points can select the same
package for a target without duplicating manifest-ancestry rules.

## Scope selection

The current directory establishes an ancestor manifest containing a `[workspace]` table, while the
target path identifies the candidate package. Both must resolve beneath the same such manifest
before another command is started. This prevents a path from silently causing work in an unrelated
checkout.

A target beneath a non-root package manifest selects the nearest such manifest. Selection does not
interpret Cargo workspace membership declarations; the invoked Cargo command remains responsible
for accepting the selected package. The workspace-root package is excluded from package selection,
so targets it owns follow the same explicit policy as targets outside every package: use workspace
scope, succeed without running the command, or fail. The selected scope is conveyed either through
Cargo package arguments or an environment variable, allowing Cargo commands and other automation
tools to share one detection model.

External execution happens only after scope validation succeeds, so a discovery failure
cannot launch a command with guessed or partial scope.

## Diagnostics and error boundary

Successful scope decisions are reported separately from failures. The CLI reports failures on
stderr and returns a failure status; a child command's status remains the outcome of that child
rather than being reclassified as a discovery failure. Diagnostics identify the attempted
workspace, manifest, path, or process operation, retain available underlying causes, and avoid
redundant category prefixes.

Internal ownership and error propagation are documented in the
[implementation guide](implementation.md).
