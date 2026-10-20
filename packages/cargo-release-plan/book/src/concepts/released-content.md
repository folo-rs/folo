# Released content

**Released content** is the content relevant to consumers of a Cargo package.
The assessment follows the files Cargo would package and the dependency
resolution an installable binary would deliver. It is not a filter over
"production-looking" directory names.

## Packaged files

Git-tracked files participate according to each package's `include` and `exclude`
rules. A path selected at either the anchor or the work tree counts, so deleted
files and files removed from an allow-list remain visible. A package is identified
by its name even when its directory moves.

Uncommitted changes to tracked files are assessed. Untracked files are reported
as advisory evidence; add intended release inputs to Git before relying on their
assessment. A nested package boundary keeps its files out of the enclosing
package's content.

The executable bit is released content as well as file bytes. Git's file-mode
handling allows the comparison to remain meaningful on filesystems without Unix
executable permissions.

Git-tracked symbolic links are assessed by their package payload rather than by
the bytes Git stores for the link itself. The link keeps its path in the package,
while a terminal file supplies content and executable mode. A terminal directory
duplicates its tracked descendants beneath the link path, matching Cargo's
archive layout. The same result is used when a Unix checkout has a native link
and when a Windows checkout materializes the link as a regular target-text
placeholder.

Supported links use UTF-8 relative targets and remain inside the selected
workspace at every hop. They terminate at a Git-tracked regular file or expand a
directory's tracked descendants recursively. A directory-link cycle is rejected
when expansion follows a link back to a directory whose expansion is already in
progress; there is no fixed nesting-depth limit. An absolute or escaping target,
missing or untracked target, unsupported filesystem object, or non-portable target
syntax is an assessment error with remediation in the diagnostic. Targets must
also be canonical relative spellings: a trailing separator or parent traversal
after a named component is rejected rather than guessed. The tool rejects
symlinked parent directories and never follows a released link into arbitrary
filesystem state.

Include/exclude rules apply to the final expanded archive paths, not only to the
bare link. Nested `Cargo.toml` boundaries are preserved through aliases and use
the manifest presence at each comparison endpoint. Links whose entire possible
archive subtree is excluded are not traversed.

The exception is specifically for released package paths. Workspace manifests,
the lockfile, Cargo configuration and untracked source inputs must remain ordinary
files. Preparation fingerprints the validated link kind, each intermediate link
and all terminal tracked files. It also records validated target directories so
an empty expansion can be recreated during preview. Changing a target or replacing
a regular file with a link invalidates previously prepared evidence.

`Cargo.toml` is packaged content. Even a formatting-only change to a package
manifest requires assessment; the tool does not maintain a second policy that
decides which lines of that file are important.

## Choose archive contents deliberately

An `include` allow-list makes the consumer build explicit and keeps new
development-only files from entering the archive automatically. For example:

```toml
[package]
include = ["src/**/*"]
```

This is a manifest fragment, not a complete package. Add any build script and
other files required by the consumer build. If source embeds a file outside
`src` at compile time, the archive must contain it, including when that code is
behind a published feature.

An inherited `include` value replaces rather than extends the workspace value.
A package with additional compile-time inputs needs a complete list of its own.
Tests, benchmarks and books can remain repository-only when consumers do not
need them. Cargo's actual rules, not this organizational recommendation, determine
what the tool assesses.

Changing an allow-list changes the package manifest and can change the archive;
it is itself a released-content change.

## Inputs Cargo adds

Cargo packages its manifest and a generated lockfile. It also includes declared
README and license files outside ordinary path rules, including inherited files
or resources outside the package directory. Without an explicit README setting,
Cargo detects a default README; `readme = false` opts out.

These inputs can affect a package even when its `include` list names only `src`.
A package-root `target` directory is not packaged.

To audit the model against Cargo, use the explicit
[`--verify-packaging` probe](../reference/commands.md#assessment). It is not part
of the normal offline check.

## Inherited workspace values

A package's published manifest contains the values it inherits from
`[workspace.package]` and `[workspace.dependencies]`. For example, changing an
inherited Rust version or dependency requirement affects every package that
inherits that value, without needing an edit under each package directory.

Attribution is per package, not a blanket release of the whole workspace.
Workspace lint configuration does not participate in this released-content
model. Path-only development dependencies are omitted by Cargo when packaging;
adding a version requirement changes that boundary.

## Binary locked dependencies

Library consumers resolve dependencies in their own graph. A library-only
package therefore does not gain a release reason merely because Cargo puts a
lockfile in its archive.

An installable binary is different: `cargo install --locked` uses its packaged
resolution. A package containing a binary, including a mixed library/binary
package, releases its **locked dependency closure**: the dependencies reachable
for installation, identified by name, version and source.

The tool compares a package-rooted closure from the workspace lockfile, not raw
workspace lockfile bytes. It is conservative: registry dependencies retain their
workspace-unified locked edges, so packaging can prune optional edges that this
assessment includes. It does not perform separate package/feature resolution
during read-only classification. Normal and build dependencies across target platforms participate;
development-only workspace edges do not. Examples, benchmarks, tests and build
scripts do not turn a library into an installable binary.

Consequently:

- An unrelated lockfile change need not release every binary.
- A transitive binary dependency change can require a release with no source
  diff in that binary package.
- Libraries still need consistent lockfiles for locked workspace commands even
  when lockfile changes are not release reasons.

Source identity matters: equal dependency names and versions do not make
different registries or Git sources interchangeable. Unresolvable identity is an
assessment error, not evidence of unchanged content.

During publication, Cargo can normalize the archive lockfile and prune inactive
dependency branches. It must not select a dependency identity outside the
installation closure that was assessed before merge.
