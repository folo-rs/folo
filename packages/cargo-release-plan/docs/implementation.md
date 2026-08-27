# cargo-release-plan implementation

User-visible classification, reporting, and plan application belong in the
package [design](design.md). This guide follows the workspace rules for
[implementation documentation](../../../docs/implementation.md).

The application is organized around a library `run()` entry that the binary and
the integration tests share. Command-line parsing lives beside that entry so
help text and parse errors can be exercised without spawning a process. The
compiled binary is also covered by a subprocess test.

## Subprocess boundaries

Every Git operation spawns the `git` executable; no in-process Git library is
used. The tool's verdicts must match what a maintainer's own `git` reports,
including whatever that installation's configuration, extensions, and repository
format imply — a library reimplements those rules and diverges from them at its
own pace, which would make a release verdict depend on a second, invisible
interpretation of the repository. Spawning also keeps repository parsing out of
this process, so a malformed or unsupported repository surfaces as a failed
command with Git's own diagnostic rather than a panic or a silent
misinterpretation here.

The trade-off accepted in exchange is real: process spawning costs more than an
in-process call, output must be parsed from text (NUL-delimited wherever Git
offers it), and `git` must be on `PATH`. Those costs are bounded because
classification runs a small number of commands per invocation, and the tool
already depends on `cargo` being present for the same kind of reason.

Package discovery for the work tree is reached only by spawning
`cargo metadata --no-deps`. `check --verify-packaging` may spawn
`cargo package --list`. `apply` may spawn `cargo update --offline`. Historical
trees are read with `git show` / `git ls-tree` rather than checking out a work
tree.

## Classification

Classification walks the base revision's first-parent commits that touch a
`Cargo.toml` (always including the base SHA and the oldest first-parent commit)
until a parsed version change (or creation) is found, then diffs released paths
between that commit and the work tree. Packaging rules are compiled once from
gitignore-style `include` / `exclude` patterns. Inherited-value attribution
reads `.workspace = true` keys out of each package manifest and compares the
corresponding tables in the root manifest at the anchor and in the work tree.
Comparison is on canonically typed TOML values rather than rendered text, so a
reformatted table is not mistaken for a changed value and two differently shaped
values never collapse into the same representation.

## Plan application

Plan application rewrites manifests structurally so comments and layout
survive: every affected manifest is parsed and patched in memory before writes
begin. A later write can still fail after earlier files have been updated.
Exact `=` pins are rewritten; other path-dependency requirements are rewritten
only when they would no longer match the new version. Registry dependencies
that happen to share a crate name are left unchanged. A `version.workspace =
true` package version is replaced with a literal version string. The lockfile
refresh is a subsequent `cargo update --offline -p …` of the rewritten
packages, skipped when the plan expands to no packages.

## Errors

Operational conditions are private leaves that flow into `ohno::AppError`.
Command, parse, and filesystem causes remain attached. The package exports
neither those conditions nor a package-specific aggregate, in accordance with
the workspace [error-handling guide](../../../docs/error-handling.md).
