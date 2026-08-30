# cargo-release-plan — Design

## Purpose

The tool exists for a workspace where merging is releasing: whatever reaches
the release branch is what consumers get from the registry. That model holds
only if every package whose published content changed also carries a version
number that has not been published yet, and nothing enforces that by reading a
diff.

A package has **unreleased changes** when the content it would publish differs
from the content of its last release. That is a statement about content alone.
Raising a version number does not change what the registry is serving today, so
it does not take a package out of this state; only releasing it does, which in
this workspace means merging.

What raising the version does decide is what the merge will do, and that is
what `cargo-release-plan` reports for every publishable package:

* A package with unreleased changes whose declared version has already been
  raised past its last release is **pending release**. The merge publishes it,
  and its changes stop being unreleased at that moment.
* A package with unreleased changes whose declared version has not moved
  **needs an increment**. A merge would ship changed content under a version
  number that is already on the registry, and nothing would publish it.

Two kinds of automation follow, and each has its own command:

* **Gating a merge.** `check` reports failure for as long as any package needs
  an increment or a version group disagrees with itself. A pull request cannot
  merge until every package whose published content it changed has been
  incremented.
* **Deciding the increments.** `report` describes what changed, per package,
  in enough detail to choose an increment level for each; `apply` then edits
  the manifests to carry that decision out.

Two terms carry the rest of this document. The **release baseline** is the
revision a workspace is compared against: the tip of the branch releases are
made from. A package's **anchor** is the commit on that baseline where the
package's declared version last changed, and is therefore the best available
record of what the currently declared version released.

## Commands

`report --out-dir <dir>` writes `report.json` and a unified diff for each
package that needs an increment on account of a file difference. The report includes
intra-workspace dependencies and dependents so version decisions can cascade.
Only edges that survive into the published manifest are reported: normal and
build dependencies cascade, as do dev dependencies that declare a version
requirement. A path-only dev dependency does not, because Cargo strips it when
it normalises the manifest for packaging.

`check` exits non-zero when any package needs an increment or any version
group is inconsistent, with one actionable diagnostic line that names the
`increment-versions` skill. `--format github` adds workflow annotations.

`check --verify-packaging` additionally cross-checks the released-content rules
against `cargo package --list` and prints warnings without changing the exit
code. It is advisory rather than authoritative because listing a package makes
Cargo require a clean tree, resolve the dependency graph, and assemble an
archive for every candidate, none of which the classification path requires;
making the verdict depend on it would turn a dirty work tree or an unavailable
registry into a release failure and would give up the offline, no-resolve
guarantee the rest of the tool provides. Each warning names the paths only the
rules claim and the paths only Cargo claims, because that is what tells the
reader whether a rule is wrong or the tree simply is not clean — an untracked
file is never released content but Cargo would still pack it. A mismatch on a
clean tree means the released-content rules and Cargo disagree, so it is
investigated and fixed in the rules, not tolerated.

`apply --plan <plan.json>` reads a plan (`schema_version` 1 with per-target
`level` or `version`), expands version groups, rewrites package versions and
intra-workspace requirements that must follow (including `=` pins in
non-publishable members), and refreshes the workspace lockfile. Manifests are
edited structurally so comments and layout survive. Every reason a plan can be
rejected — an unknown target, a version that would move backwards, an unreadable
manifest — is decided before the first manifest is written, so a rejected plan
changes nothing. `--dry-run` reports the manifests that would change and writes
nothing. Writes themselves are sequential, so a plan that is accepted and then
fails on an I/O error can leave earlier manifests updated; the work tree is
reverted with the version control system rather than by the tool.

All three read the workspace named by `--manifest-path`, defaulting to the
manifest discovered from the working directory, and take the release baseline
from `--base`.

## The release baseline

The baseline is the tip of the branch releases are made from. Passing it
explicitly is the reliable choice, because only the caller knows which branch
that is: it is a property of how the project publishes, not of the revision
being examined. In particular it is not necessarily the branch a pull request
targets. A stacked pull request targets its parent branch, which has released
nothing; comparing against that parent would call content released that no
registry has ever seen. Continuous integration should therefore name the
release branch itself, as a ref or a resolved SHA.

Without `--base` the tool asks Git which branch the `origin` remote considers
its default and uses that remote-tracking branch, falling back to `origin/main`
only when the remote publishes no default. Both are conveniences for running by
hand; neither knows whether the default branch is the release branch.

Nothing about the comparison requires a pull request. Running on the release
branch itself is the ordinary way to ask which packages are pending release and
would be published by the next release run: the baseline is then an ancestor of
the work tree, so every package that was incremented since it is reported as
pending release. Running with a work tree that has uncommitted edits is equally
supported, because the comparison reads the work tree rather than a commit;
this is what makes `check` useful before committing.

## Anchors

A package needs an increment when its released content differs between its
**anchor** and the work tree while its declared version has not increased since
that anchor.

The anchor is the most recent commit on the **baseline's first-parent line** in
which the package's parsed `version` field changed. Comparison uses the parsed
version, so reformatting a manifest without changing the version is not an
increment. A package's creation — absent to present — counts as a version
change.

A truncated history that never reveals a version change, creation included, is
an error rather than a pass: a clone shallow enough to hide the anchor cannot
support a claim that nothing needs releasing.

### Anchors across merges

The walk follows first parents, which makes each merged pull request one step
in the baseline's history rather than a detour through the commits that
composed it. Two properties follow, and they are what make the anchor stable.

On a linear baseline the walk visits every commit, and the anchor is simply the
most recent one that changed the version:

```
  A --- B --- C --- D        baseline first-parent line
        ^           ^
        |           baseline tip
        version 0.2.0 declared here; B is the anchor
```

When work reaches the baseline through a merge, the walk steps over the merged
commits and lands on the merge itself:

```
              E --- F               topic branch, second parent of M
             /       \
  A --- B --- C ------ M --- D      baseline first-parent line
                       ^
                       0.2.0 first reaches the baseline here; M is the anchor
```

`F` is where somebody typed the new version, but `M` is where that version and
its content became what the baseline publishes. Anchoring on `M` is what makes
the anchor a statement about released history instead of about the order in
which topic branches happened to be written.

A branch under examination can also merge the baseline into itself, which is
the reverse direction and does not disturb anything:

```
  A --- B ------------ M --- D          baseline first-parent line
         \                    \
          P --- Q ------------ N --- R  branch being examined
```

The anchor is resolved on the baseline alone, so `N` is not a candidate however
recent it is. What `N` changes is the work tree: after it, the branch holds the
baseline's newer releases as well as its own commits, so the comparison against
the anchor shows the branch's own changes and nothing else.

A branch that has *not* merged the baseline is compared against the same anchor
and its work tree simply lacks whatever the baseline released in the meantime.
Those differences are reported too, because they are real: the branch would
publish content older than what is already released. Merging the baseline in
resolves it.

### Packages the baseline does not publish

A package the baseline does not publish — because it does not exist there, or
exists with `publish = false` — has no released content to compare against, so
the branch is preparing its first release and it is pending release whatever
its version.

A name that was published once, deleted, and later restored is not treated as a
continuation of that earlier incarnation. Deciding which past release a restored
directory continues would rest the verdict on commits the baseline no longer
carries and a shallow clone need not fetch at all. Reconciling a restored name
with what the registry already holds under it is left to whoever restores it.

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
* **The build directory.** A `target` directory at the package root is never
  released content, because Cargo drops it before it reads `include` or
  `exclude`. A directory of that name deeper in the package is ordinary source.
* **Symbolic links.** A link among a package's released content stops the run.
  Cargo dereferences a link when it builds a package archive, so the published
  bytes are the target's content, while Git records only the target's path;
  comparing what Git stores would call a package unchanged after an edit to the
  file it points at. Replace the link with a regular file, or keep it out of the
  released content with `exclude`.

### Lockfiles of binary packages

Cargo writes a lockfile into every package archive, but only a package that
publishes an executable releases what that lockfile says. `cargo install
--locked` builds from it, so the exact dependency versions it names are part of
what the consumer receives; for a library the consumer resolves their own
versions and the archived lockfile never participates in a build. A change to
the resolved dependencies of a package with a binary target is therefore a
released content change, and the same change against a library is not — the
same criterion that keeps `[workspace.lints]` out of scope, applied to a
different input.

What is compared is the package's own dependency closure as resolved by the
workspace lockfile, not the lockfile's bytes: Cargo re-derives the archived
lockfile from the closure it needs, so unrelated members of the same workspace
moving does not reach the archive. The package's own entry is excluded from its
closure, so incrementing a binary package is never itself a further change to
that package. A workspace with no lockfile contributes nothing to the
comparison rather than an invented difference.

A dependency update that touches a binary's closure marks that binary as needing
an increment even though no file in it was edited. This is deliberate: the
executable really would be built from different code. It becomes pending release
once it is incremented, because its declared version has then moved past the
anchor.

A `Cargo.lock` nested deeper inside a package, rather than the one governing the
workspace, is ordinary source and is compared like any other file.

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

| Status            | Condition                                                |
| ----------------- | -------------------------------------------------------- |
| `pending-release` | version increased since the anchor                       |
| `needs-increment` | version unchanged, released content changed since anchor |
| `released`        | version unchanged, released content unchanged            |

`check` reports failure when any package is `needs-increment`; the other two
statuses are states a release branch can merge from. A `pending-release` package
still holds unreleased changes — it is merging that releases them — but the
merge will publish them under a version number that is not yet on the registry,
which is what the gate is there to guarantee.

`publish = false` packages are excluded. Group consistency is a separate
group-level condition: a package may need an increment *and* belong to an
inconsistent group, and both are reported.

### Version groups

Members of a version group share a declared version. If any member needs an
increment, all members increment. The new version is the highest version
declared by any member, raised by the highest required level. Members the
baseline does not carry are exempt from the consistency rule, so a new package
can join a group before it has been published. A member the baseline carries but
does not publish is not exempt: it exists there and may already have been
released before it was withdrawn.

Group membership is `[workspace.metadata.release-plan.groups]` in the workspace
root manifest. A plan entry names either a package or a group, so a group named
after a workspace package must contain that package; naming it after a package
it does not contain is rejected as ambiguous configuration. A group may only
name publishable packages: a group keeps released versions in lockstep, so a
`publish = false` member has no released version to keep in step and is rejected
rather than quietly left out of every decision.

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