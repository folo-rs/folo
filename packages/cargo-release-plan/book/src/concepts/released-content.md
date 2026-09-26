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
A package-root `target` directory is not packaged. Released symbolic links need
attention: Git stores the link target path while Cargo packages target bytes, so
the ordinary historical comparison cannot establish their released contents.

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

The tool compares this package-specific closure, not raw workspace lockfile
bytes. Normal and build dependencies across target platforms participate;
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
