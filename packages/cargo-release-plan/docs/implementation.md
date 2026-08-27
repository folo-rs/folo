# cargo-release-plan implementation

User-visible classification, reporting, and plan application belong in the
package [design](design.md). This guide follows the workspace rules for
[implementation documentation](../../../docs/implementation.md).

The application is organized around a library `run()` entry that the binary and
the integration tests share. Command-line parsing lives beside that entry so
help text and parse errors can be exercised without spawning a process. The
compiled binary is also covered by a subprocess test.

Every module owns a subject rather than a category, so the crate has no shared
bag of types or constants: the run input and outcome sit with `run()`, the check
format and the skill named in failure text sit with the check command, the schema
revision sits with plan parsing, and the default base revision sits with argument
parsing. The public surface is re-exported from the crate root, so where an item
is defined is free to follow its subject.

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

Paths live in one of two spaces and cross between them at exactly one place.
Git names every path relative to the repository root and separates directories
with `/` on every platform, so a path Git reports is taken verbatim: `\` is an
ordinary character in a file name, and rewriting one would name a file that does
not exist or collide two distinct files onto one key. Operating-system paths
arrive from `cargo metadata`, and the manifest layer converts them into Git's
space by rewriting only the platform's own separator. Everything downstream of
that conversion — packaging rules, diffs, plans, diagnostics — is in Git's
space.

Path listings are decoded from Git's raw bytes and any name that is not valid
UTF-8 stops the run. A file name on Unix is an arbitrary byte string, so this is
reachable for an ordinary tracked file; decoding lossily would substitute the
offending bytes, which can collapse two distinct paths onto one entry and makes
every subsequent read of that path address a different file. A wrong release
verdict is worse than a refusal to give one. Output that is not a file name is
still decoded lossily, because there a substituted byte only affects a message.

Every child inherits a locale pinned to `C`. Git translates its diagnostics, and
the one place where a Git failure is interpreted rather than surfaced — telling
"this path is absent from that revision", a routine outcome for a package created
or deleted on the branch, apart from an operational failure — has no
machine-readable signal to read and must match Git's wording. Under a translated
locale that match would fail and ordinary package creation would abort the run.
Pinning the locale at the shared boundary also keeps captured output identical
between a maintainer's terminal, a CI runner, and the test harness, which is the
same reason colour is disabled there.

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
commit. `members` and `exclude` globs are matched root-anchored, as Cargo
resolves them against the workspace root. Beyond the declared `members` patterns,
Cargo makes every path dependency of a member that lives inside the workspace a
member as well, so that closure is followed to a fixed point and still honours
`exclude`. A path dependency inherited through `[workspace.dependencies]` is
followed too, resolved against the workspace root rather than the member
directory, because that is where the root manifest declares it.

A package's released content stops at a nested package boundary, and those
boundaries are read off the tracked manifests beneath the package rather than off
the member list. Cargo stops packing at any directory that carries its own
`Cargo.toml`, whether or not the workspace claims it, so a fixture crate the
workspace excludes would otherwise have its files attributed to the enclosing
package and produce unreleased-change verdicts for content that is never
released. Each side of the comparison reads its own boundaries from its own file
listing, so a crate that appears or disappears between the anchor and the work
tree moves the boundary with it.

The work-tree side narrows that listing further. Git keeps reporting a tracked
file the work tree has deleted, but Cargo packages what is on disk: a nested
manifest that is gone no longer stops packing, and a deleted default README is
no longer there to detect. Eligibility still comes from the tracked listing —
released content is defined from git-tracked files — while these structural
questions are answered from the paths that still exist.

Released content also reaches past what the packaging rules describe, because
Cargo packs the files named by `readme` and `license-file` regardless of
`include` and `exclude`, and from outside the package directory if that is where
they live. Those are located by manifest key rather than by pattern, so they are
resolved in classification rather than in the packaging rules. A locally
declared value resolves against the package directory and an inherited one
against the workspace root, matching where each manifest declares it. Cargo
keeps a resource that is already inside the package at its own path and flattens
one from outside into the crate root, so the resolved set is keyed the same way
and reproduces the layout Cargo produces; where the ordinary file listing has
already claimed a key, that claim wins, as it does in Cargo.

Because a resource may sit outside the package directory, the per-directory
listing that establishes released content cannot vouch for it, so the work-tree
side asks Git for the tracked state of every resource path in one batched query.
Reading it off disk instead would let an untracked file decide a release
verdict, which the git-tracked rule forbids. The anchor side needs no such
query: reading a resource back from a commit that did not track it already
yields nothing.

A symbolic link is refused rather than compared, because neither reading is
right at both ends: Cargo dereferences a link when it packs, while Git stores
the target's path as the blob. The work-tree side detects one from the file
type, and the anchor side from the tree's mode, which is the only place the
distinction survives once a blob is read. Both look at the released paths only,
and the anchor listing covers the manifest resources alongside the package
directory, so a link that only history holds is caught as well as one on disk.

Content is compared by Git object identity rather than by bytes. Git converts
content on its way into the object database — line-ending rules and clean
filters such as Git LFS both apply — so a file on disk and the blob recording it
routinely hold different bytes even when nothing has changed. The anchor's ids
come from the tree listing that already establishes released content, and the
work tree's from hashing the released files, which applies the same conversion
staging them would. Delegating the question that way means no conversion has to
be understood, reimplemented, or kept in step with Git. Bytes are then read only
for the paths whose ids differ, to render the patch.

Historical manifests are read without Cargo's help, so every `.workspace = true`
key a member declares is resolved against the root manifest of the same commit.
`version`, `include`, `exclude`, and `publish` all matter to classification: with
them unresolved a member would be read with Cargo's defaults and get the wrong
anchor, the wrong released-file set, or no exclusion at all.

Because Cargo opens member directories through the filesystem while Git reports
the spelling recorded in the tree, member matching follows the case rules the
workspace directory actually applies. Those rules are probed once per run by
re-opening an existing entry under a case-flipped spelling; an inconclusive
probe yields case-sensitive matching, which never widens membership.

A non-virtual root's own package is a member whatever `members` and `exclude`
say, so that case is decided before the patterns are consulted.

Package directories reported by `cargo metadata` are workspace-relative, but
every pathspec handed to Git is repository-relative, so directories are rebased
onto the repository root before use. The prefix they are rebased onto comes from
Git, which reports it alongside the repository root in the same `rev-parse` that
discovers the repository. Subtracting the root from the workspace path Cargo
reported would instead compare two independent spellings of one directory, and
those need not match: Windows hands out 8.3 short names for some paths, and both
tools accept uncanonical spellings. The workspace root manifest is located the
same way, so a nested workspace's historical snapshots are never reconstructed
from the repository-root manifest. Git rejects an empty pathspec, so
the repository root — which every one of these paths spells as the empty string —
becomes `.` inside the Git wrapper rather than at each call site.

A package that is absent from the base revision is only treated as newly created
once the first-parent walk shows no sampled commit carried it. When some earlier
commit did, the branch is restoring a package, so the walk resumes at the newest
commit that carried it and then applies the ordinary anchor rule. Anchoring on
that commit directly would be wrong whenever content was committed without an
increment before the deletion: the anchor would absorb the unreleased content and
a package restored at the same version would look released. Treating every
absence as creation would instead let a restored package re-publish a version
that is already on crates.io.

The non-gating packaging cross-check compares Cargo's own `cargo package --list`
against exactly the released-content selection classification uses, rather than
against a set rebuilt from `include` and `exclude`. A second implementation of
the same rules would drift from the first and warn about packages whose rules are
right — a README Cargo detects for itself, or a crate nested inside the package —
which is the opposite of what the cross-check exists to surface.

## Diagnostics

Every repository-controlled name a diagnostic prints — a path, a package or group
name, an inherited field, a dependency table a plan rewrites, a manifest a plan
writes, a value an error condition names — goes through one shared quoting helper
that borrows Git's `core.quotePath` rendering: a name carrying a quote,
backslash, or control character is wrapped in quotes with those bytes escaped.
Control means every Unicode control, not only the ASCII ones, because a C1
control such as U+009B drives a terminal exactly as its ASCII counterpart does;
printable non-ASCII characters are left alone so an accented file name stays
readable. Plain lines are read from a CI log or a terminal, so an unescaped
newline could let the tail of a name pose as a fresh workflow command and an
unescaped escape sequence could rewrite what a reader sees. The explanatory
`--verbose` notes are held to the same rule as the verdict text, since both land
on the same stderr. A subprocess's own stderr is the single exception: `git` and
`cargo` already quote the names they report, and escaping their whole diagnostic
would fold a multi-line explanation onto one unreadable line.

`--format github` renders each diagnostic as a workflow command in addition to
the plain line. The message body is additionally escaped for `%` and line breaks
and property values for `:` and `,`, because those delimit the command itself
rather than the name inside it.

## Plan application

Plan application rewrites manifests structurally so comments and layout
survive: every affected manifest is parsed and patched in memory before writes
begin. A later write can still fail after earlier files have been updated.
Exact `=` pins are rewritten; other path-dependency requirements are rewritten
only when they would no longer match the new version. Every workspace member's
manifest is visited, not just the publishable ones: a `publish = false` member
can pin a package the plan increments, and a stale pin left behind would break
the lockfile refresh. Registry dependencies
that happen to share a crate name are left unchanged. A `version.workspace =
true` package version is replaced with a literal version string. The lockfile
refresh is a subsequent `cargo update --offline --workspace`, skipped when the
plan expands to no packages or the workspace has no `Cargo.lock`. `--workspace`
rather than a `-p` spec per rewritten package: a bare package name is an
ambiguous package-ID spec whenever the lockfile also holds a registry package of
that name, and the manifests are already written by then, so the failure would
leave the work tree applied against a stale lockfile.

## Errors

Operational conditions are private leaves that flow into `ohno::AppError`.
Command, parse, and filesystem causes remain attached. The package exports
neither those conditions nor a package-specific aggregate, in accordance with
the workspace [error-handling guide](../../../docs/error-handling.md).
