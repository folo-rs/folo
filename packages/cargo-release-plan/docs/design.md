# cargo-release-plan - Design

`cargo-release-plan` classifies every publishable workspace package against its
version anchor and applies an approved increment plan. A package has unreleased
changes when its released content differs between its version anchor and the
work tree without an increase in its declared version. A base revision is valid
for release when no publishable package is in that state and every version group
is consistent, which are the two conditions `check` reports on.

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

A run without `--base` takes the base revision from the workspace's
`[workspace.metadata.release-plan] base` key, and falls back to `origin/main`
when the workspace declares none. CI should pass an explicit SHA of the
merge-base or target-branch tip. A stale default can both add and hide
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

Released content is the git-tracked source a package publishes. Three rules
define it:

* **Git decides what exists.** Only git-tracked files are released content, and
  they are compared as Git itself stores them, so the line-ending rules and
  clean filters a repository configures — Git LFS among them — never make an
  untouched package look changed and never hide an edit. Untracked files are
  reported as an advisory and never counted as changes.
* **The package's own rules decide what is selected.** The package's `include` /
  `exclude` keys select from the files under its directory using gitignore-style
  matching, so a directory pattern covers everything beneath it. Those keys are
  honoured whether the package declares them itself or inherits them from
  `[workspace.package]`, as is `publish`: an inherited `publish = false` excludes
  a package from classification exactly as a locally declared one does.
* **A nested manifest ends the package.** A package releases nothing from inside
  a directory that carries its own `Cargo.toml`, whether or not that directory
  is a workspace member, matching the package boundary Cargo itself stops at.

The change set is a diff from the anchor to the work tree. The package directory
is resolved independently at each end from that end's workspace member list,
keyed by package name, so a relocated package is still compared with itself.
Comparison is package-relative: a move that leaves released file bytes unchanged
is `released`. A path that either end would package participates in the
comparison, which is how a deleted packaged file or a path dropped from
`include` is visible. Every released file counts, so a comment-only or
formatting-only edit to a packaged `Cargo.toml` is a released content change.

A file's executable bit counts as well, because Cargo carries the mode Git
records into the archive it builds. A packaged file that becomes executable
without an edit is therefore a released content change, reported against the
file with the `old mode` and `new mode` headers Git itself uses. The mode is
read from the index rather than the filesystem, so the same commit classifies
identically on a platform that has no executable permission to observe.

### Where Cargo departs from those rules

Cargo packages a few things the general rules would not select, and
classification follows it in each case:

* **Manifest-named resources.** The files named by `readme` and `license-file`
  are released content wherever they live and whatever the packaging rules say,
  because Cargo packs each regardless of `include` and `exclude`. One declared
  from outside the package directory lands at the archive root under its file
  name, and one from inside keeps its own path. A workspace-level README that
  several packages inherit is therefore released content for every one of them,
  and editing it in place is a change to each; so is editing a README the
  package's own `include` leaves out. The git-tracked rule still applies: an
  untracked resource is reported as an advisory, under the path it would take
  inside the package archive, and never counted as a change.
* **The default README.** A package that declares no `readme` at all still
  releases one, because Cargo probes the package directory for its own default
  names and packs the first it finds. That file is released content on the same
  terms as a declared one, and an `include` that omits it changes nothing.
  `readme = false` opts out, and `readme = true` names Cargo's preferred
  default.
* **The package's own `Cargo.lock`.** It is never released content, because
  Cargo derives the published lockfile when it builds the archive. A lockfile
  nested deeper in the package is ordinary source.
* **The build directory.** A `target` directory at the package root is never
  released content, because Cargo drops it before it reads `include` or
  `exclude`. A directory of that name deeper in the package is ordinary source.
* **Symbolic links.** A link among a package's released content stops the run.
  Cargo dereferences a link when it builds a package archive, so the published
  bytes are the target's content, while Git records only the target's path;
  comparing what Git stores would call a package unchanged after an edit to the
  file it points at. Replace the link with a regular file, or keep it out of the
  released content with `exclude`.

### Path case

Path case is modelled once per workspace, from the volume holding the workspace
root, and applied to member resolution and default-README probing everywhere
beneath it. A workspace whose subdirectories disagree about case — a
per-directory setting on Windows, or a case-insensitive mount grafted under a
case-sensitive root — is outside that model. Content read out of history always
uses the exact spelling Git recorded, so the model only decides which spellings
a workspace member or a default README may answer to, never which bytes are
compared.

### Inherited workspace values

An inherited value participates in classification when it can change what a
consumer receives in the published package archive. Cargo resolves
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

### Version groups

Members of a version group share a declared version. If any member needs an
increment, all members increment. The new version is the highest version
declared by any member, raised by the highest required level. Members that do
not exist on the base revision are exempt from the consistency rule, so a new
package can join a group before it has been published. A member the base
revision carries but does not publish is not exempt: it exists there and may
already have been released before it was withdrawn.

Group membership is `[workspace.metadata.release-plan.groups]` in the workspace
root manifest. A plan entry names either a package or a group, so a group named
after a workspace package must contain that package; naming it after a package
it does not contain is rejected as ambiguous configuration. A group may only
name publishable packages: a group keeps released versions in lockstep, so a
`publish = false` member has no released version to keep in step and is rejected
rather than quietly left out of every decision.

## Commands

`report` writes `report.json` and a unified diff for each package whose
unreleased changes include a file difference. The report includes intra-workspace dependencies and dependents so
version decisions can cascade. Only edges that survive into the published
manifest are reported: normal and build dependencies cascade, as do dev
dependencies that declare a version requirement. A path-only dev dependency does
not, because Cargo strips it when it normalises the manifest for packaging.

`check` exits non-zero on unreleased changes or an inconsistent group, with one
actionable diagnostic line that names the `increment-versions` skill.
`--format github` adds workflow annotations.

`--verify-packaging` cross-checks the released-content rules against
`cargo package --list` and prints warnings without failing the check. It is
advisory rather than authoritative because listing a package makes Cargo require
a clean tree, resolve the dependency graph, and assemble an archive for every
candidate, none of which the classification path requires; making the verdict
depend on it would turn a dirty work tree or an unavailable registry into a
release failure and would give up the offline, no-resolve guarantee the rest of
the tool provides. Each warning names the paths only the rules claim and the
paths only Cargo claims, because that is what tells the reader whether a rule is
wrong or the tree simply is not clean — an untracked file is never released
content but Cargo would still pack it. A mismatch on a clean tree means the
released-content rules and Cargo disagree, so it is investigated and fixed in
the rules, not tolerated.

`apply` reads a plan (`schema_version` 1 with per-target `level` or `version`),
expands version groups, rewrites package versions and intra-workspace
requirements that must follow (including `=` pins in non-publishable members),
and refreshes the workspace lockfile.
Manifests are edited structurally so comments and layout survive. Every reason a
plan can be rejected — an unknown target, a version that would move backwards,
an unreadable manifest — is decided before the first manifest is written, so a
rejected plan changes nothing. `--dry-run` reports the manifests that would
change and writes nothing. Writes themselves are sequential, so a plan that is
accepted and then fails on an I/O error can leave earlier manifests updated; the
work tree is reverted with the version control system rather than by the tool.

### Report artifacts

`report.json` records every classified package, its status, its anchor, its
change set, and the group verdicts, and is the complete verdict on its own. Each
entry in `changed[]` carries the source of the change, so an inherited-value
change is recorded there rather than in a diff. A package whose only change is
an inherited value therefore has no patch, which is why the report rather than
the `diffs/` directory is what a consumer enumerates.

The per-package `.patch` files are zero-context unified diffs in the shape
`diff -U0` produces, so consumers can pipe them into standard tooling: one hunk
per changed region, `/dev/null` labelling the absent side of an addition or
deletion, and binary content reported as differing rather than rendered. An
added or deleted file whose content is empty has no hunk to carry it, so it is
recorded with Git's extended `new file` / `deleted file` headers, which a patch
reader applies.

A patch is a readable rendering of a difference the report has already recorded,
so it is refined only while refining is cheap. Two sides that differ by more
than a bounded number of lines are rendered as a whole-file replacement — every
old line removed, every new line added — which remains a correct patch and still
identifies the file as differing, just without a per-region breakdown. No verdict
depends on which of the two renderings a file receives.

## Offline and deterministic

Classification never contacts a registry, never resolves a dependency graph, and
never compiles anything, so it produces the same verdict without a network and
without a warm build. Verbose logging explains the inputs and rules behind each
decision, not merely the conclusion.

Internal ownership is documented in the [implementation guide](implementation.md).
