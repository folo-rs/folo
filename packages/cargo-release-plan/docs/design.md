# cargo-release-plan - Design

`cargo-release-plan` classifies every publishable workspace package against its
version anchor and applies an approved increment plan. The version increment is
the release event: on a healthy base branch, no publishable package has released
content sitting past its most recent parsed `version` change.

## The invariant

A package fails when its released content differs between its **anchor** and the
work tree, while its declared version has not increased since that anchor.

The anchor is the most recent commit on the **base revision's first-parent
line** in which the package's parsed `version` field changed. Walking
first-parent means each merged pull request counts as one step. Reading the
anchor off the base revision, not the working branch, means a branch's own
commits never become anchors. Comparison uses the parsed version, so
reformatting a manifest without changing the version is not an increment. A
package's creation (absent to present) counts as a version change.

The base revision defaults to `origin/main`. CI should pass an explicit SHA of
the merge-base or target-branch tip. A stale default can both add and hide
differences, so it is not a conservative fallback.

A truncated history that never reveals a version change (including creation) is
an error, not a pass.

## Released content

Released content is the git-tracked files Cargo would put in the `.crate`: the
`git ls-files` set filtered by the package's `include` / `exclude` using
gitignore-style matching. `Cargo.lock` is never released content. Untracked
files are reported as an advisory and never counted as changes.

The change set is a diff from the anchor to the work tree. The package
directory is resolved independently at each end from that end's workspace
member list, keyed by package name, so a relocated package is still compared
with itself. Comparison is package-relative: a move that leaves released file
bytes unchanged is `released`. A path that either end would package
participates in the comparison, which is how a deleted packaged file or a path
dropped from `include` is visible.

A comment-only or formatting-only edit to a packaged `Cargo.toml` is a released
content change.

### Inherited workspace values

Values a package actually inherits from `[workspace.package]` or
`[workspace.dependencies]` are in scope when those values changed between the
package's anchor and the work tree. Attribution is per package: only inheriting
packages are marked. `[workspace.lints]` is out of scope.

A global inherited change therefore marks every inheriting package.

## Package status

| Status               | Condition                                                | Verdict  |
| -------------------- | -------------------------------------------------------- | -------- |
| `releasing`          | version increased since the anchor                       | pass     |
| `unreleased-changes` | version unchanged, released content changed since anchor | fail     |
| `released`           | version unchanged, released content unchanged            | pass     |

`publish = false` packages are excluded. Group consistency is a separate
group-level verdict: a package may have unreleased changes *and* belong to an
inconsistent group, and both are reported.

Members of a version group share a declared version. If any member needs an
increment, all members increment. The new version is the highest version
declared by any member, raised by the highest required level. Members that do
not exist on the base revision are exempt from the consistency rule, so a new
crate can join a group before it has been published.

Group membership is `[workspace.metadata.release-plan.groups]` in the workspace
root manifest.

## Commands

`report` writes `report.json` and per-package unified diffs for unreleased
changes. The report includes intra-workspace dependencies and dependents so
version decisions can cascade.

`check` exits non-zero on unreleased changes or an inconsistent group, with one
actionable diagnostic line that names the `increment-versions` skill.
`--format github` adds workflow annotations. `--verify-packaging` is non-gating:
it cross-checks relevance rules against `cargo package --list` and prints
warnings without failing the check.

`apply` reads a plan (`schema_version` 1 with per-target `level` or `version`),
expands groups, rewrites package versions and intra-workspace requirements that
must follow (including `=` pins), and refreshes the workspace lockfile.
Manifests are edited structurally so comments and layout survive. The complete
edit set is computed before any write. `--dry-run` lists the manifests that
would change, without writing.

## Offline and deterministic

Classification uses only `git` and `cargo metadata --no-deps`. It does not
contact crates.io, resolve a dependency graph, or compile. Verbose logging
explains the inputs and rules behind each decision, not merely the conclusion.

Internal ownership is documented in the [implementation guide](implementation.md).
