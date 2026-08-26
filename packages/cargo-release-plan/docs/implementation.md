# cargo-release-plan implementation

User-visible classification, reporting, and plan application belong in the
package [design](design.md). This guide follows the workspace rules for
[implementation documentation](../../../docs/implementation.md).

The application is organized around a library `run()` entry that the binary and
the integration tests share. Command-line parsing lives beside that entry so
help text and parse errors can be exercised without spawning a process. Git is
reached only by spawning `git`; package discovery for the work tree is reached
only by spawning `cargo metadata --no-deps`. Historical trees are read with
`git show` / `git ls-tree` rather than checking out a work tree.

Classification walks the base revision's first-parent line until a parsed
version change (or creation) is found, then diffs released paths between that
commit and the work tree. Packaging rules are gitignore-style matching via the
`ignore` crate. Inherited-value attribution reads `.workspace = true` keys out
of each package manifest and compares the corresponding tables in the root
manifest at the anchor and in the work tree.

Plan application is a `toml_edit` rewrite: every affected manifest is parsed
and patched in memory, then written only after the full edit set succeeds. Exact
`=` pins and any intra-workspace requirement that would no longer match the new
version are updated in package tables and in `[workspace.dependencies]`. The
lockfile refresh is a subsequent `cargo update --offline -p …` of the rewritten
packages.

Operational conditions are private leaves that flow into `ohno::AppError`.
Command, parse, and filesystem causes remain attached. The package exports
neither those conditions nor a package-specific aggregate, in accordance with
the workspace [error-handling guide](../../../docs/error-handling.md).
