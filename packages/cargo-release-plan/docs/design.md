# cargo-release-plan - Design

`cargo-release-plan` classifies every publishable workspace package against its
version anchor and applies an approved increment plan. A package has unreleased
changes when its released content differs between its version anchor and the
work tree without an increase in its declared version; a base revision is valid
for release when no publishable package is in that state.

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

### Version monotonicity

Versions only ever move forwards. A declared version *below* the version the
anchor released is an error rather than a status, because the lower version was
already published with different content and republishing it is impossible. The
same rule applies to `apply`: a plan that names an explicit version lower than a
target already declares is rejected.

## Released content

Released content is the git-tracked files Cargo would put in the `.crate`: the
`git ls-files` set filtered by the package's `include` / `exclude` using
gitignore-style matching. `Cargo.lock` is never released content. Untracked
files are reported as an advisory and never counted as changes. A package that
contains another workspace member releases nothing from inside it, matching the
package boundary Cargo itself stops at.

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

An inherited value participates in classification when it can change what a
consumer receives in the published `.crate`. Cargo resolves
`[workspace.package]` and `[workspace.dependencies]` inheritance at package
time, so those values are baked into the published manifest and a change to them
is a change to released content. `[workspace.lints]` is out of scope by the same
criterion: lint configuration governs how this workspace compiles its own
sources and is not part of what a consumer builds against, so an inherited lint
change cannot alter the released package.

Values a package actually inherits from `[workspace.package]` or
`[workspace.dependencies]` are in scope when those values changed between the
package's anchor and the work tree. Attribution is per package: only inheriting
packages are marked.

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
package can join a group before it has been published.

Group membership is `[workspace.metadata.release-plan.groups]` in the workspace
root manifest.

## Commands

`report` writes `report.json` and per-package unified diffs for unreleased
changes. The report includes intra-workspace dependencies and dependents so
version decisions can cascade. Only edges that survive into the published
manifest are reported: normal and build dependencies cascade, dev dependencies
do not.

`check` exits non-zero on unreleased changes or an inconsistent group, with one
actionable diagnostic line that names the `increment-versions` skill.
`--format github` adds workflow annotations.

`--verify-packaging` cross-checks the relevance rules against
`cargo package --list` and prints warnings without failing the check. It is
advisory rather than authoritative because `cargo package` needs a clean tree, a
resolvable dependency graph, and a full pack of every candidate, none of which
the classification path requires; making the verdict depend on it would turn a
dirty work tree or an unavailable registry into a release failure and would give
up the offline, no-resolve guarantee the rest of the tool provides. A reported
mismatch means the relevance rules and Cargo disagree about released content, so
it is investigated and fixed in the rules, not tolerated.

`apply` reads a plan (`schema_version` 1 with per-target `level` or `version`),
expands groups, rewrites package versions and intra-workspace requirements that
must follow (including `=` pins), and refreshes the workspace lockfile.
Manifests are edited structurally so comments and layout survive. The complete
edit set is computed before any write. `--dry-run` lists the manifests that
would change, without writing.

### Report artifacts

`report.json` records every classified package, its status, its anchor, its
change set, and the group verdicts. Each entry in `changed[]` carries the source
of the change, so an inherited-value change is recorded there rather than in a
diff.

The per-package `.patch` files are zero-context unified diffs in the shape
`diff -U0` produces, so consumers can pipe them into standard tooling: one hunk
per changed region, `/dev/null` labelling the absent side of an addition or
deletion, and binary content reported as differing rather than rendered. A file
whose presence changed but whose content is empty is recorded by its headers
alone, because there are no lines to show.

## Offline and deterministic

Classification uses only `git` and `cargo metadata --no-deps`. It does not
contact crates.io, resolve a dependency graph, or compile. Verbose logging
explains the inputs and rules behind each decision, not merely the conclusion.

Internal ownership is documented in the [implementation guide](implementation.md).
