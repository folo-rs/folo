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
offers it), and `git` must be on `PATH`. The cost is proportional to the work
rather than fixed — reading historical content spawns one `git show` per file,
so it grows with the size of the released content and the length of the walked
history. Batching those reads through a single long-lived `git cat-file --batch`
process would remove that growth and is the optimization to reach for first if
invocation time becomes a problem; it is not taken today because the per-file
form is the one whose failure modes map directly onto Git's own diagnostics.

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

Historical workspace membership is reconstructed from the root manifest at each
commit. Beyond the declared `members` patterns, Cargo makes every path
dependency of a member that lives inside the workspace a member as well, so that
closure is followed to a fixed point and still honours `exclude`. Membership is
tracked for non-publishable members too: `cargo package` stops at a nested
package boundary, so a package that contains another member must not claim the
inner member's files as its own released content.

Because Cargo opens member directories through the filesystem while Git reports
the spelling recorded in the tree, member matching follows the case rules the
workspace directory actually applies. Those rules are probed once per run by
re-opening an existing entry under a case-flipped spelling; an inconclusive
probe yields case-sensitive matching, which never widens membership.

A non-virtual root's own package is a member whatever `members` and `exclude`
say, so that case is decided before the patterns are consulted.

## Diagnostics

`--format github` renders each diagnostic as a workflow command in addition to
the plain line. Package names, paths, and version strings all reach those
commands from the repository, so message bodies are escaped for `%` and line
breaks and property values additionally for `:` and `,` — otherwise a legal but
awkward path could truncate an annotation or begin a second command.

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
