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

A package that is absent from the base but present at some earlier commit on
the base's first-parent line is being restored, not created. The version it
declared before the deletion may already be published, so its anchor is the last
version change on that history and the ordinary monotonicity and content
comparisons apply.

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
gitignore-style matching, so a directory pattern covers everything beneath it.
Those keys are honoured whether the package declares them itself or inherits
them from `[workspace.package]`, as is `publish`: an inherited `publish = false`
excludes a package from classification exactly as a locally declared one does.
The package's own `Cargo.lock` is never released content, because Cargo derives
the published lockfile at pack time; a lockfile nested deeper in the package is
ordinary source. Untracked
files are reported as an advisory and never counted as changes. A package
releases nothing from inside a directory that carries its own `Cargo.toml`,
whether or not that crate belongs to the workspace, matching the package
boundary Cargo itself stops at.

A symbolic link among a package's released content stops the run. Cargo
dereferences a link when it packs a `.crate`, so the published bytes are the
target's content, while Git records only the target's path; comparing what Git
stores would call a package unchanged after an edit to the file it points at.
Replace the link with a regular file, or keep it out of the released content
with `exclude`.

Released content is compared as Git itself stores it, so the line-ending rules
and clean filters a repository configures — Git LFS among them — never make an
untouched package look changed, and never hide an edit.

The files named by `readme` and `license-file` are released content wherever
they live and whatever the packaging rules say, because Cargo packs each
regardless of `include` and `exclude`. One declared from outside the package
directory lands at the crate root under its file name, and one from inside keeps
its own path. A workspace-level README that several packages inherit is
therefore released content for every one of them, and editing it in place is a
change to each; so is editing a README the package's own `include` leaves out.
The git-tracked rule applies to a resource as it does to anything else: an
untracked one is reported as an advisory, under the path it would take inside
the `.crate`, and never counted as a change.

A package that declares no `readme` at all still releases one: Cargo probes the
package directory for its own default names and packs the first it finds, so
that file is released content on the same terms as a declared one and an
`include` that omits it changes nothing. `readme = false` opts out, and
`readme = true` names Cargo's preferred default. That probe goes through the
filesystem, so on a volume that ignores path case a differently cased spelling
answers a default name and is released.

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
root manifest. A plan entry names either a package or a group, so a group named
after a workspace package must contain that package; naming it after a package
it does not contain is rejected as ambiguous configuration. A group may only
name publishable packages: a group keeps released versions in lockstep, so a
`publish = false` member has no released version to keep in step and is rejected
rather than quietly left out of every decision.

## Commands

`report` writes `report.json` and per-package unified diffs for unreleased
changes. The report includes intra-workspace dependencies and dependents so
version decisions can cascade. Only edges that survive into the published
manifest are reported: normal and build dependencies cascade, as do dev
dependencies that declare a version requirement. A path-only dev dependency does
not, because Cargo strips it when it normalises the manifest for packaging.

`check` exits non-zero on unreleased changes or an inconsistent group, with one
actionable diagnostic line that names the `increment-versions` skill.
`--format github` adds workflow annotations.

`--verify-packaging` cross-checks the relevance rules against
`cargo package --list` and prints warnings without failing the check. It is
advisory rather than authoritative because `cargo package` needs a clean tree, a
resolvable dependency graph, and a full pack of every candidate, none of which
the classification path requires; making the verdict depend on it would turn a
dirty work tree or an unavailable registry into a release failure and would give
up the offline, no-resolve guarantee the rest of the tool provides. Each warning
names the paths only the rules claim and the paths only Cargo claims, because
that is what tells the reader whether a rule is wrong or the tree simply is not
clean — an untracked file is never released content but Cargo would still pack
it. A mismatch on a clean tree means the relevance rules and Cargo disagree
about released content, so it is investigated and fixed in the rules, not
tolerated.

`apply` reads a plan (`schema_version` 1 with per-target `level` or `version`),
expands groups, rewrites package versions and intra-workspace requirements that
must follow (including `=` pins in non-publishable members), and refreshes the
workspace lockfile.
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
deletion, and binary content reported as differing rather than rendered. An
added or deleted file whose content is empty has no hunk to carry it, so it is
recorded with Git's extended `new file` / `deleted file` headers, which a patch
reader applies.

## Offline and deterministic

Classification uses only `git` and `cargo metadata --no-deps`. It does not
contact crates.io, resolve a dependency graph, or compile. Verbose logging
explains the inputs and rules behind each decision, not merely the conclusion.

Internal ownership is documented in the [implementation guide](implementation.md).
