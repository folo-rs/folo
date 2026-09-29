# cargo-release-plan - Design

## Purpose

`cargo-release-plan` implements a release process where reviewed version
increments merged to a designated **release branch** publish their packages. It
supplies evidence for choosing increments, applies the complete version decision
before merge, and publishes the resulting crates, GitHub releases and prebuilt
binaries afterward.

Tag creation that the workflow cannot complete is an explicit manual-recovery
case. The workflow reports the failure and the operator action needed to resume;
it does not prevent further merges or acquire broader tagging authority.

The application supplies the mechanical operations throughout this process.
People still judge compatibility and approve changes; workflows supply execution
and permissions. Version validation and publication share one release model,
not separately configured release tools. A reusable GitHub Action and workflows
expose these operations to other repositories without requiring a Folo checkout
or repository-local release scripts.

The `increment-versions` skill is the supported version-planning interface; the
CLI operations are its protocol, not an alternative manual planning workflow.

Version planning also supports an explicitly requested standalone run. The user
supplies the workspace, release-history ref and optional merge target instead of
a publication configuration. The same assessment and application operations
prepare a version-increment PR without adopting publishing workflows.

The product is a command-line application, not a Rust library API. Its supported
interfaces are the CLI and documented configuration and artifact formats.
Library targets in the application and its implementation packages exist only for executable
wiring and maintainer tests. Changes to those internal Rust types do not by
themselves require a breaking release; changes to the supported interfaces still
receive their normal compatibility assessment.

This document describes the behavior of the product and how its participants
fit together. The public user guide teaches adoption and operation; the
implementation guide describes internal construction.

## The release process

### From a source change to a delivered release

**Released content** means the content relevant to consumers of the Cargo package,
including an installable binary's locked dependencies. Not every repository edit
changes released content.

A **version assessment** establishes which released content changed and whether
the declared versions cover those changes. Its **release history** ends at a
selected commit on the branch that publishes.
Within that history, each package has its own **anchor**, the newest first-parent
commit that changed its parsed version. The anchor supplies the package's
anchor version and content. The anchor version is the version declared by the
package in that commit. For a stacked PR, a separate **merge target**
identifies the unmerged parent's final state. Its higher package versions and
final contents are treated together as anticipated predecessor releases.

A **semantic impact** describes the significance of the changed consumer contract:
`breaking`, `nonbreaking` or `patch`. A **version plan** translates those decisions
into package versions, including required group and dependency effects. The
resulting resolved plan fixes the manifest and lockfile edits to apply.

A **non-publishable package** declares `publish = false`: it participates in version-group alignment
without registry publication or a semantic impact about its source. A
publishable private-API package is different; it still has released content and
publication obligations.

The process connects these concepts as follows:

1. Prepare a consistent workspace lockfile and collect an assessment using
   the selected history and merge target. Review source changes, inherited inputs, dependency effects
   and external compatibility-check results.
2. Choose semantic impacts, generate a version plan and preview its complete
   effects. Assess additional effects exposed by preview before applying the
   captured result.
3. Review and squash-merge the source changes together with their version changes.
   Automated merge checks verify version readiness and supported compatibility
   requirements; human review supplies the approval.
4. Prepare a publication manifest from the clean, merged source. This manifest
   fixes which package versions to deliver, not which versions to choose.
5. Reconcile crates.io, then package tags and binary releases, then native binary
   assets. Each phase observes remote state and completes missing work.

For example, adding a compatible operation to a library calls for a `nonbreaking`
semantic impact. The tool determines the appropriate version movement and
propagates any related manifest or binary-lockfile effects. After review and merge,
publication delivers those declared versions without reconsidering the decision.

### Responsibilities

| Participant | Responsibility |
| --- | --- |
| Maintainer or authoring agent | Understand the package's promises, judge behavioral and semantic compatibility, and explain the chosen levels. |
| `cargo-release-plan` | Collect released-content evidence; validate version rules; compute and apply mechanical plan effects; validate publication inputs; reconcile and publish the chosen versions. |
| API compatibility checker (`cargo-semver-checks`) | Supply evidence about supported Rust library contracts; it does not cover every behavioral, CLI or data-format promise. |
| `increment-versions` skill | Guide the authoring agent through evidence collection, semantic impacts, preview, application and verification. It uses the tool's operations rather than implementing another version resolver. |
| Reviewer and repository merge policy | Approve the complete contribution and require its automated checks. Completing the skill is not approval to merge or publish. |
| GitHub Action and reusable workflows | Install the selected tools, select event-specific inputs, provide jobs and permissions, transport artifacts and expose outcomes. They do not independently choose package versions or release policy. |
| Cargo | Resolve dependencies when explicitly requested, construct and verify crate archives, and perform workspace registry uploads with Cargo's dependency-ordering semantics. |

Thus "the tool does not infer API compatibility" describes the semantic boundary,
not an absence of compatibility support. Selecting packages to compare, invoking
the API compatibility checker in an explicit operation, and enforcing effects
of an already-known breaking version are mechanical operations. Deciding whether
an undocumented behavior change breaks a promise remains the author's task.

### Version state is not publication state

* **Pending release** means the declared version is greater than its anchor's
  version, or the package is preparing its first release.
* **Needs an increment** means released content changed without advancing the
  version beyond its anchor.
* **Unchanged** means released content and version still match the anchor.

These states answer a pre-merge question: does this source have the version
movement required by release history? They do not answer whether an upload
succeeded. For example, after version `1.5.0` merges, that merge can be the new
anchor. Assessing the merged source reports `unchanged` even if its crates.io
upload is still waiting or failed.

Publication answers the separate delivery question by checking exact versions and
assets remotely. It includes all publishable packages declared in the validated
source, not only the assessment's pending-release subset.

Semantic impacts are distinct from numeric **version bumps** (`patch`,
`minor`, `major`). For stable versions, `nonbreaking` normally requires a minor
increment and `breaking` a major increment. Pre-1.0 compatibility rules and
already sufficient pending increments affect that translation. An exact target
version can be chosen when numeric increments do not express the intended release.
For example, a `nonbreaking` decision for `0.4.2` can require a numeric `patch`
increment to `0.4.3`. The artifact reference distinguishes semantic
`changes[].impact` from numeric `increments[].bump`. These are distinct fields
because the meaning of a change is not its numeric version operation.

Version choices belong before merge. Publication never chooses another version,
repairs a manifest or refreshes the committed dependency resolution. Preparation
of a publication is an automatic execution step, not a second human release gate.

### Scope

Standalone version planning ends with the reviewed version-increment PR being
merged under the repository's normal authorization policy. It requires neither
`release_plan.toml` nor registry bootstrap, Trusted Publisher setup or workflow
installation. Publishing is outside this mode; the reusable workflow remains the
canonical supported publication path.

The supported publication path is Cargo workspaces following this version model,
crates.io through Trusted Publishing, and GitHub tags, releases and native binary
archives. Libraries receive package tags; packages with an installable binary
also receive a GitHub release and prebuilt assets. Binary publication supports one
executable per package. Cross-compilation, alternative registries or forges,
changelog generation and arbitrary release-policy hooks are outside this scope.

## Design tenets

### The release branch is the source of truth

Actual release history comes from the branch that publishes. An unmerged PR target
is a separately identified anticipated squash release, not existing release history.
Version assessment retains this distinction for stacked PRs. Remote publication state
determines which delivery operations remain, not which versions are valid.

### One selected history, package-specific comparisons

Every package uses the same release-history commit and optional merge target
throughout an assessment. Package-specific anchors preserve catch-up work in
existing history. Pending versions introduced by the target instead compare
against its final content, so independently mergeable child PRs cannot reuse a
parent's increment. Refreshing either input requires fresh assessment.

### Published artifacts decide relevance

The question is not whether files in a package directory changed. It is whether
the content Cargo would publish changed. Package rules, inherited manifest
values, executable bits, manifest-named resources, package boundaries, and
installable binary lockfile closures therefore participate where they affect
the consumer's build. Physical inclusion of a lockfile alone does not make its
contents release-relevant.

### Evidence and judgement stay separate

Released-content analysis determines whether an increment is required and records
the evidence. Semantic impacts combine that evidence with
external compatibility results and wider contract assessment. Proposal generation completes their mechanical
version effects; resolution preview exposes additional dependency-resolution
effects before application. A minimum imposed by a tool is not a complete semantic
assessment.

### Consumer contracts

A published package does not necessarily offer an API for consumers to use. An
implementation partition exists to serve the public package in front of it, and
some packages are published only because Cargo requires a dependency to be
published. Both still have `pub` items, so nothing in the code distinguishes
them from a package meant for direct use: it is a promise the publisher makes,
and the package declares it.

```toml
[package.metadata.release-plan]
private-api = true
```

A package is public unless it declares itself private, and a package with no
library target presents no contract either way. That direction is chosen for its
failure mode rather than its frequency: a package wrongly treated as public
produces a finding a maintainer can act on, while one wrongly treated as private
produces nothing at all. A malformed declaration is an error for the same reason.

A CLI package whose library target exists only for application and maintainer-test
wiring declares `private-api = true` as well. Its CLI and artifact contracts still
receive the normal compatibility assessment; internal Rust visibility does not
make those types a supported library interface.

This declaration selects evidence for the API compatibility checker; it
does not ask `cargo-release-plan` to infer whether an API changed.
`semver-targets` reads the report's consumer-contract flags and chooses the public
library contracts that need comparison. Assessing an implementation partition directly would
measure a surface no consumer can reach, and demand version increases of the
public package for changes its consumers cannot observe. A re-exported item
appears in the public package's own documented API and is compared there.

For example, if `widget_impl` and `widget` share a version group and only
`widget_impl` declares `private-api = true`, an implementation change selects
`widget` for library API comparison. Both packages still participate in
released-content assessment and version alignment. The author separately judges
behavioral effects; a package with no library target can still have a breaking CLI
change that the API compatibility checker cannot detect.

### Public dependencies

A dependency is **public** when the dependent's own public API exposes types
from it, whether by re-exporting them or by naming them in a signature. The
distinction matters because a public dependency's compatibility is part of the
dependent's own contract.

Which dependencies are public is read from the dependent's
`allowed_external_types` allow-list rather than inferred from source. That
allow-list names every type outside the crate that its public API may expose,
and `check-external-types` fails the build when the API exposes one the list
omits, so in a passing workspace the list is a superset of what is genuinely
exposed. Reading a declaration the repository already verifies keeps this
offline and avoids a second, weaker inference of the public API.

This verification is an adoption prerequisite, not something ordinary `check`
proves. The consuming repository runs `cargo-check-external-types` as a required
API-validation check for its supported feature and platform surfaces. An omitted
allow-list declares no external exposure; it is not permission to leave exposure
unknown. The public integration guide includes this check alongside the
repository's build and test gates, without relying on a Folo Just recipe.

The allow-list names the crate that *defines* a type, which is not always the
dependency that supplies it: a package usually reaches an implementation crate's
types re-exported through the public crate in front of it. The re-exporting
crate closes that gap, because it must declare the crate it re-exports in its
own allow-list. Following those declarations transitively attributes a named
crate to the direct dependency that actually supplies it. Only a normal
dependency qualifies, since a build or development dependency cannot supply
types to a library's public API.

Two consequences follow, and the tool enforces both:

* An intra-workspace requirement names the exact version its target declares.
  A requirement that merely admits the target's version lets a consumer resolve
  a combination the workspace never built. A path-only development dependency
  escapes packaging and is not assessed by this release rule.
* A package whose public dependency releases a semver-incompatible version
  must release one as well. Such a release changes the identity of the exposed
  types, so a consumer holding the older dependency can no longer hand its
  types to the dependent. This follows from the version move alone, however
  unrelated the dependency's breaking change was to the items actually exposed.

Only the second is a release decision. A requirement whose form is wrong is
corrected by editing the requirement, not by incrementing anything.

### The release decision is offline and reproducible

The normal released-content assessment path uses only repository history, the work tree, and
`cargo metadata --no-deps`. It never contacts a registry, resolves the full
dependency graph, or compiles packages. The same inputs therefore produce the
same classification without network or build-cache state.

Compatibility checking is a separate, explicit operation: its checker can compile
packages and retrieve a published comparison version. Online publication is also
an explicit command family. Neither becomes a prerequisite of ordinary offline
`report` or `check`.

### Publication reconciles immutable intent

A publication fixes package versions and source identities before writing remote
state. Retrying observes what already succeeded and completes only missing work.
A failed query cannot establish absence, and a successful earlier phase cannot
establish that later phases completed.

Partial publication is not rolled back. Existing package versions and release
tags remain authoritative; recovery does not overwrite them to make a run appear
complete. Cargo owns package construction and registry upload mechanics, while
the application enforces the release model and reconciles delivery.

### Validation stays proportional to its purpose

Package authors own buildability and the prerequisites of their selected releases.
Cheap input checks can explain mistakes, but the application does not reproduce
Cargo, Git or remote-service behavior merely to report a failure earlier.
Nontrivial validation must protect a release-specific guarantee, such as the
correspondence between source and published content, safe application of reviewed
edits, or accurate delivery and recovery state. It is not a promise that every
unsuccessful release can be rejected before publication starts.

### Rejected plans do not edit manifests

Plan targets, version direction, and group expansion are validated before any
manifest is written. Files are then edited structurally so comments and layout
survive.

### Resolution precedes application

Dependency resolution is explicit preparation, not part of classification or
application. The intended offline workspace refresh precedes semantic assessment.
Prospective version and requirement changes are resolved before application as well:
resolving unchanged manifests alone cannot predict their effects.

The proposal settles version groups, requirement propagation, and binary
dependency-closure effects internally. It retains adequate existing increments
instead of repeatedly increasing a package at each resolution pass. The captured
state includes resolved file contents and the inputs they depend on. Application
uses that state without a late dependency refresh or unlisted package changes.
Changed inputs require fresh preparation and assessment.

## Commands

### Standalone planning

The skill has a configured mode and an explicitly requested standalone mode.
Configured mode reads publication configuration and retains its publication
readiness gates. Standalone mode takes the selected Cargo workspace, actual
release-history ref and any anticipated parent ref from the user or the user's
explicitly supplied task context. It must not infer permission to operate on an
unrelated repository merely because configuration is missing.

Standalone planning passes resolved commits through `--release-history` and
`--merge-target`; it does not synthesize publishing configuration or call
`release-context`. The original refs are retained for refresh before application.
Preparation, semantic impacts, dependency/group expansion, preview and captured
application keep their ordinary contracts. The final version-readiness check
omits `--config`, so binary publication metadata is not a prerequisite.

Registry identity checks and the managed first-publication handoff belong to
configured publication, not to standalone version edits. API compatibility
comparison remains separate evidence: its historical comparison can still read
crates.io, and an unavailable comparison does not establish compatibility.
Standalone operation does not add a different registry or publishing backend.

### Produce evidence for versioning decisions with `report`

`report --out-dir <dir>` produces the evidence used to choose versions. It
writes a machine-readable package report and readable patches for packages whose
files changed. Dependency and dependent relationships are included so a
compatibility decision can account for changes that propagate through the
workspace.

Only relationships preserved in the published manifest are relevant. Normal and
build dependencies participate, as do development dependencies with a version
requirement. Cargo removes a path-only development dependency when packaging, so
it does not propagate a release decision.

### Protect a release with `check`

`check` is intended for a merge gate. It fails while any package needs an
increment, a version group disagrees with itself, an intra-workspace
requirement does not name the version its target declares, an exact
intra-workspace requirement is malformed, or a package that exposes a public
dependency stays compatible while that dependency releases a breaking change.
It points the maintainer to the `increment-versions` skill that prepares a plan.

`--format github` additionally emits GitHub Actions error annotations. These are
structured log records that attach each failure to the affected package
manifest, so the workflow summary and pull-request file view make the problem
visible without reading the raw log.

With publication configuration supplied through `--config`, `check` also validates
publication inputs: target selection, the single-binary requirement and
`cargo-binstall` metadata. This remains an offline check. The standard reusable
check workflow supplies that configuration explicitly and treats a missing file
as an error; ordinary assessment without it retains its version-checking scope.
The lower composite exposes version readiness separately, so a repository can
keep a deliberately narrower merge-queue check without weakening its full PR gate.

`check --verify-packaging` audits the tool's artifact model against
`cargo package --list`. It warns when Cargo and the tool select different paths
but does not alter the version-readiness result. The probe allows dirty trees, so
untracked inputs may legitimately appear only on Cargo's side. It also performs
dependency resolution and Cargo's package preparation work, which the normal
offline assessment deliberately avoids. A mismatch on a clean tree is evidence
that the artifact model needs correction.

### Version-planning stages

A version plan exists in two stages, and they carry different guarantees about the
packages a document names.

A **proposed plan** is generated from the skill's semantic impacts. Its entries may name a version
group, or a single member of one, and leave resolution to reach the rest, so what
it names is a starting point rather than the full set it moves.

The **resolved plan** produced by preview names every package whose
version the plan sets and records the version each will carry. Both halves
matter: the first makes the documented set complete with respect to the release
decision, and the second makes it stable, since a version bump would be
resolved again against whatever the manifests say when the document is applied.
Resolving its package versions must therefore reproduce them exactly.

Applying a plan also rewrites the requirements that dependents declare on the
packages it moves. A dependent whose existing pending increment is sufficient
need not receive another one. A dependent that would otherwise keep its
anchor's version needs its own release decision before application.

The resolved plan is applied unchanged, so the documented package/version set
and applied document are the same artifact. Review and approval policy belong
to the caller, not the tool.

### Plan from captured evidence

`analysis-order`, `semver-targets`, and `propose` operate entirely on report
artifacts. Their answers do not depend on a checkout, a registry, or installed
compatibility tools.

Semantic assessment is dependency-first. Every publishable package appears in an
analysis batch, with mutually dependent packages assessed together. Version groups
do not create artificial dependency cycles, and non-publishable members do not
receive semantic assessments.

Compatibility target selection follows consumer contracts. A changed package
selects the public contracts in its version group rather than demanding a
comparison of private implementation APIs. Packages without changed released
content do not independently select a comparison.

Proposal generation consumes explicit `breaking`, `nonbreaking`, or `patch`
decisions. It retains adequate pending version increases, aligns version groups
without regression, and propagates required dependent releases. Public dependency
breaks use the same compatibility rule as the release gate. Requirements rewritten
by the proposal cannot leave a dependent at its anchor's version.
These mechanical requirements do not replace semantic judgement.

The proposal is based on the report's declared versions and release anchors.
Preview remains responsible for resolving prospective manifests and lockfiles;
its additional evidence can require a fresh semantic impact.

### Collect external compatibility evidence

`check-compatibility` consumes prepared inputs or a resolved plan's retained
prospective workspace, or collects fresh read-only report evidence. It uses the report-selected consumer contracts and runs
the supported API compatibility checker. The operation records comparison inputs,
checker identity, findings and diagnostics; it does not replace the author's
semantic impacts.

Prepared execution explicitly selects a workspace and verifies its captured
source, selected history, merge target and resolution. Fresh execution captures those inputs around
its own report generation. A matching HEAD
alone is insufficient for a dirty work tree. The tool verifies those inputs before
and after the comparison, as it does for a retained preview. Artifact-only target
selection does not by itself establish that the selected checkout matches a report.
The comparison versions and any anticipated-parent source are recorded with the
result; an unavailable comparison is not silently replaced with a different one.

A self-comparison canary checks that the installed checker can perform a comparison
before its evidence is relied upon. Findings, a valid empty target set and an
execution failure remain distinct outcomes. Missing or incomplete comparison
evidence is never reported as compatibility. When checking a preview, the tool
verifies that evidence collection left its captured source and resolution intact.
The shared workflow uses the result to enforce supported API compatibility;
the skill uses it as a floor while assessing the complete contract.

### Check first-publication prerequisites

`check-published` is an explicit registry-read operation, separate from offline
plan inspection. For a resolved plan, it validates the complete target set and
checks whether its publishable packages are established on crates.io. Non-publishable
packages require no registry observation. Never-published packages and indeterminate
queries block this pre-application gate; neither is silently treated as published.

Workspace-wide discovery provides an early advisory handoff before a plan exists.
This check does not publish first versions or prove that Trusted Publisher
registration is configured. Those remain maintainer setup responsibilities.

### Inspect a plan

`inspect-plan` validates a plan against the selected workspace and provides
publication-eligible target names and any retained compatibility manifest.
Non-publishable packages remain part of validation but not publication.
Requiring resolved evidence applies the same captured-state checks as a dry-run
application. Inspection performs no writes or registry queries; external callers
can use it independently of the publication commands' availability checks.

### Prepare evidence and preview resolution

Preparation supplies a consistent workspace lockfile and evidence describing that
same source, before semantic impacts are made.

Preview applies candidate versions and requirement rewrites in a disposable
workspace and resolves there under the same offline policy. It classifies the
prospective tree using the selected history and merge target and expands release effects
until versions and captured manifest/lockfile contents are both stable.
A repeated non-final state is a resolution error, not a completed preview.
Transitive binary lockfile effects and
re-selection among already-locked dependency versions therefore appear before
application, not as a request for a second versioning pass.

Automatically required releases are visible in the final proposal and its
evidence. They establish minimum release requirements, not a claim of semantic
compatibility: the caller assesses newly exposed dependency changes and raises
levels when the package's contract requires it, then previews again before
applying the stable proposal.

### Carry out a decision with `apply`

`apply --plan <plan.json>` applies only a resolved plan using the exact
state captured by preview. It validates the input
snapshot and target set before installing the captured manifest and lockfile
contents. It does not run dependency resolution. An already-applied resolved
plan is an idempotent no-op; a partially changed or stale input is not treated as
the captured state. `--dry-run` reports what would change without writing.

An unresolved proposal cannot be applied, including in dry-run mode.

### Between report and apply

Choosing an increment level requires comparing a change with the package's
contract. The report supplies the changed files, inherited values, locked
dependencies, and workspace relationships needed for that judgement. It does
not compile code, compare API surfaces, or infer compatibility from a textual
diff.

After a person or an agent records the choices in a plan, preview accounts for
the mechanical consequences. It expands version groups, derives new versions,
rewrites requirements that must follow, and resolves the lockfile before the
complete result is applied. Post-application verification confirms that result;
it is not a routine source of additional lockfile-only release decisions.

Workspace commands use the workspace selected by `--manifest-path`. Artifact-only
planning commands instead use their supplied reports and decisions. `report` and
`check` accept `--release-history` and optional `--merge-target` to identify the
comparison history.

### Prepare and execute publication

Publication uses a **publication manifest**: the validated source identity and
exact package/version requests for a run. It is not a proposed or expanded version
plan, and does not require retaining a pull request's planning artifacts.

| Command | Role |
| --- | --- |
| `prepare-publish` | Validate the release snapshot and write its publication manifest without changing source or remote state. |
| `publish registry` | Publish missing crate versions and confirm their registry availability. |
| `publish github` | Reconcile package tags and binary GitHub releases, then emit platform batches for incomplete assets. |
| `publish binaries` | Build and publish one platform batch from its immutable tag commits. |

The manifest passes between phases as an artifact. A **platform batch** is the
set of binary releases one native target must complete. Each phase validates its
input identity and refreshes the remote state on which its decisions depend.
Registry and GitHub reconciliation support a read-only `--dry-run` to expose
current blockers and intended writes without exchanging publication credentials
or changing remote state. A preview is not a completion receipt; missing tags
cannot yield authoritative tag-bound build batches until actually established.
Detailed source, completion and recovery guarantees are in
[Publication](#publication).

## Release history and the merge target

Release history answers which package versions are established by the publishing
branch. The selected commit is the common starting point for per-package anchor
searches, not itself every package's content comparison point.

The merge target answers what will precede the current PR. A target already in
release history adds no anticipated release. An unmerged parent is considered as
one prospective squash commit: its final tree supplies every pending version's
content, ignoring the parent's internal commit ordering. Additional child changes
must advance that version. Packages retaining an existing release-history version
continue to use its anchor, preserving unversioned catch-up changes.

For example, `main` records package `1.4.0`. A parent PR increments to `1.5.0` and
then adds another fix. A child compares with the complete parent `1.5.0`, not the
commit where its version string was first edited. Extra child changes need a
version above `1.5.0`. After the parent is squash-merged, that final content and
version are recorded together, preserving the comparison.

Squash merging is the supported managed publication workflow. Existing history
may contain migration or manual commits without matching increments; anchors
retain their outstanding released-content changes. This tolerance does not turn
an unmerged parent's intermediate commits into established releases.

`release-context` fetches the configured release branch and resolves the optional
target ref. All assessment stages use that same returned commit pair.
`--release-history <commit>` explicitly selects known release history;
`--merge-target <commit>` supplies an anticipated parent.
Direct assessment without an explicit history uses `origin`'s recorded default
branch, falling back to `origin/main`; automation should use the configured context.

The original PR checks establish independent version decisions. A merge queue can
check its candidate against the appropriate history/target inputs, but grouping
PRs is not a substitute for those checks and does not define their version scope.
No synthetic queue history is treated as a sequence of already published PRs.

When release history or the target advances, refresh assessment before applying.
Another PR may have consumed a selected version, or the parent may have changed
the child's comparison. Do not reassign old evidence to new inputs.

Version assessment supports dirty work trees, so `check` can find a missing
increment before edits are committed. Publication instead requires a clean,
pinned release snapshot. Its **publication source** identifies the merged files
whose declared versions are to be delivered. A **tag target** identifies the
immutable commit a package tag actually names and from which its binaries build.
The tag target can be a later **release-equivalent commit**: an eligible
first-parent descendant of the publication source retaining the requested package
version and released content. Neither identity replaces the history and target
selected for pre-merge assessment.

The API compatibility checker's comparison source is separate. Existing releases
normally use a published crate version; an anticipated parent uses its final
source where needed to assess the child's change in supported API.

## Anchors

An anchor is the newest commit on the selected release history's first-parent line where the
package's parsed version changed. Reformatting the version declaration does not
move it. The commit that first adds a package counts as a version change.

Squash merging records the PR's final content and version as one release event.
For historical non-squash merge commits, first-parent traversal still follows the
commit where a version reached the release branch, not the topic commit where it
was typed:

```text
          E ---- F
         /        \
A ---- B ---------- M ---- D   <- release first-parent history
                       ^
                       version reaches the release branch; M is the anchor
```

A shallow history that hides a required version change cannot support a release
claim, so the command fails rather than treating the package as unchanged.

### Packages without a comparison release

A package absent from release history and the anticipated parent, or present there
with `publish = false`, has no comparison release. It is treated as preparing its
first release and is pending release at any declared version.

A package name that was published, removed, and later restored is also treated
as new. Guessing which old incarnation it continues would make clone depth a
correctness input. Whoever restores the name must reconcile it with versions
already present in the registry.

### Version monotonicity

Versions move forward relative to the selected release line. A version below the
anchor's version is an error because that release line already records the
higher version.

Publishing a patch for an older series remains possible by using a separate
release branch based on that series. For example, `1.3.1` can follow `1.3.0` on a
maintenance branch even when another release branch has already reached `1.4.0`;
the maintenance branch supplies its own release history and anchors.

## Released content

Released content is the git-tracked content Cargo would place in the package
artifact:

* Git decides which files exist and how clean filters and line endings identify
  their content. Untracked files are advisory only.
* The package's `include` and `exclude` rules select paths beneath the package.
  `Cargo.lock` at the package root is excluded from this file comparison.
* A nested `Cargo.toml` ends the enclosing package, whether or not the nested
  package is a workspace member.
* The package directory is resolved independently at the anchor and in the work
  tree, so moving a package does not break its identity.
* The executable bit is content because Cargo preserves it in the artifact. Git's
  configured work-tree model decides whether an unstaged mode change is visible;
  the index remains the fallback where file modes are not supported.

A path selected at either end participates. Deleted files, files dropped from an
`include` list, and formatting-only edits to a packaged manifest therefore remain
visible.

Only filesystem absence is interpreted as missing released content. Operational
failures while inspecting or reading tracked inputs stop assessment.

### Where Cargo adds content

Cargo includes several inputs outside ordinary package rules:

* A declared `readme` or `license-file` is included even when rules exclude it,
  including a resource inherited from `[workspace.package]` or located outside
  the package directory.
* Without a `readme` declaration, Cargo detects a default README in the package
  directory. `readme = false` opts out.
* A package-root `target` directory is never included.
* Symbolic links are unsupported and reported as such when encountered in release inputs.

### Relevant lockfile closures

Cargo includes a generated lockfile in every package artifact. The lockfile does
not constrain consumers of a library-only package: those consumers resolve the
library in their own dependency graph. Its dependency changes are therefore not
released content for this purpose.

An installable binary target makes its package's recorded dependency resolution
release-relevant, including when that package also contains a library. Examples,
benchmarks, tests, and build scripts do not qualify, even when they are executable
or physically included in an archive.

Automatic target discovery uses tracked, present regular files. Untracked files,
deleted inputs and directories named like binary sources do not qualify.

The package-specific closure is compared rather than the workspace lockfile's
bytes, so unrelated dependency movement does not affect every binary
package. Entries are identified by name, version, and source. The root package
is selected by its name and declared version, and excluded from its own closure
so incrementing it does not create another change.

The closure covers installation dependencies, including normal and build
dependencies across target platforms. Development-only dependency edges of
workspace members do not participate, either at the binary root or through a
transitive workspace dependency.

Dependency identity includes its source, so a same-named development dependency
from another source does not enter an installation closure. Workspace patches,
registry configuration, and Cargo-supported legacy dependency tables participate.
If source identity cannot be reconstructed without guessing, assessment stops
instead of reporting the dependency as unchanged or irrelevant.

Target shape is resolved independently at the anchor and in the work tree. An
endpoint with an installable binary target requires a workspace lockfile that
resolves the package at the version declared there. An endpoint without one
contributes an empty closure and requires no lockfile. This makes adding the
first binary compare an empty anchor closure with the current
resolution, while removing the last one compares the historical resolution with
an empty work-tree closure. If a required closure cannot be reconstructed, the
assessment stops rather than treating unknown released content as unchanged. A
new package has no anchor artifact to compare and is classified as new without a
historical closure.

### Inherited workspace values

Values a package inherits from `[workspace.package]` or
`[workspace.dependencies]` are part of its published manifest. A changed
inherited value therefore affects each package that uses it.

Cargo omits a versionless dev dependency from the published manifest. Changes to
such a dependency's inherited workspace entry do not affect released content
while the entry remains versionless; adding or removing its version does.

`[workspace.lints]` is not published behavior and does not participate.

### Path case

Filesystem lookups, including workspace membership, target discovery, nested
package boundaries and historical Cargo inputs, follow the probed behavior of the
workspace directory rather than an operating-system assumption. Git-tracked
spellings remain distinct in reports so a case-only rename stays visible.
Filesystem identity does not make Cargo's packaging patterns or reserved-name
comparisons case-insensitive.

## Package status

| Status            | Meaning                                                      |
| ----------------- | ------------------------------------------------------------ |
| `pending-release` | Version is above the anchor, or the package has no anchor      |
| `needs-increment` | Released content changed without a version increase           |
| `unchanged`       | Released content and version still match the anchor            |

Of these statuses, only `needs-increment` fails `check`; the manifest-level
requirement and public-dependency rules fail it independently of status.
`publish = false` packages are excluded. None of these states proves registry,
tag or binary-asset availability.

## Version groups

Every Git-tracked Cargo workspace package participates, including a package that
cannot be published. An exact dependency declaration between two packages states
that their versions move together. Version groups are
the connected components formed by those declarations, in either dependency
direction, and contain at least two members. A group's key is its
lexicographically smallest member.

All normal, build, and development declarations participate, including optional
and target-specific declarations. An inherited declaration uses the effective
workspace dependency. A dependency alias follows the package identity it names,
and a local path must resolve to that workspace member. Registry dependencies,
outside or excluded paths, versionless paths, and unused workspace dependency
entries do not form groups.

The accepted exact form is one `=major.minor.patch` comparator, with
insignificant whitespace allowed. A partial exact version, a prerelease or build
suffix, or a compound requirement containing an exact comparator is a manifest
error. A well-formed exact requirement whose version is stale still forms its
group: `check` reports the stale requirement and `apply` can repair it.

If one publishable member needs an increment, the plan includes every package
in its group. A plan may also target a non-publishable package directly,
and groups containing only such targets can be aligned without publication. The target
starts from the highest declared member version, including non-publishable and
new members, and applies the highest chosen increment level. Entries that expand
to the same group must all use increment levels or all use one matching exact
version.

Preview records the complete package/version set rather than leaving group
members implicit.

An inconsistent group is a check failure in its own right, independent of any
content change. A plan entry naming any member resolves it, and expansion is
plan-driven, so a group no entry names is left alone. An entry that carries an
increment level raises the group's highest declared version. An entry that
carries that highest version as an exact target instead moves lagging members up
to it and leaves the leading member unchanged. The lagging members then become
pending release because their declared versions advanced.

Members absent from the comparison history are exempt from the consistency check, which
lets a new package join a group before its first release. This exemption does
not remove the member from alignment or from the version base. A member that
exists in that history with publication disabled is not absent.

The obsolete `[workspace.metadata.release-plan.groups]` key is rejected. Group
membership is declared only by exact workspace dependency requirements.

## Report artifacts

`report.json` is the complete machine-readable assessment. Its `packages` array
records every publishable package, its status and anchor, the reasons it changed,
and its dependencies and dependents. Its `non_publishable_packages` array
records each remaining package's name, declared version, and group.
Group records cover the union of both arrays and report complete consistency.

Per-package patch files are a readable supplement for file changes. They cover
every package whose released files differ from its anchor, including one whose
version has already moved, because judging whether a pending increment still
covers the accumulated changes needs the same evidence. Changes that are not
file differences — inherited workspace values and locked dependency identities —
are reported only as change entries. They use zero-context
unified diffs, report binary changes without rendering binary bytes, and
preserve addition, deletion, and mode information. Expensive line-level
comparisons fall back to a whole-file replacement; this changes only the
presentation, never the version-readiness result.

### A version-planning example

The following sketches omit schema details and captured file bytes; they
illustrate roles, not documents to submit to the CLI. A library `widget` and its
private implementation `widget_impl` form a version group. Both start at `1.4.0`.

```text
prepared assessment
  release-history commit: E
  widget:
    anchor: B, version 1.4.0
    declared version: 1.4.0
    status: needs-increment
    evidence: new public operation
  widget_impl:
    anchor: B, version 1.4.0
    declared version: 1.4.0
    status: needs-increment
    evidence: implementation of that operation
  group: [widget, widget_impl]

analysis order
  assess widget_impl before widget
compatibility targets
  [widget]

author's semantic impacts
  widget: nonbreaking
  widget_impl: patch

proposed version plan
  widget group: version 1.5.0

resolved expanded version plan
  widget: version 1.5.0
  widget_impl: version 1.5.0
  captured inputs: original prepared source, release history and merge target
  captured edits: both manifests, affected requirements, resolved Cargo.lock
  prospective evidence: report, patches and retained compatibility workspace
```

The proposal can name a group once; the resolved plan names every package. If preview exposes a binary's changed dependency closure, that package
also appears in the resolved effects and receives semantic assessment before
application. A non-publishable package appears for alignment, not upload.

`prepared.json` binds the original inputs. `report.json` explains their released
changes. The author's decisions are the semantic input to `propose`.
`preview/plan.json` captures the complete applicable result. The preview report and external
compatibility results justify the final choice. Fresh post-application evidence
verifies it without overwriting the evidence used to make it.

After merge, publication does not consume these local planning files. It reads
the reviewed versions from the merged repository and creates a different artifact
for a different purpose.

## Publication

### Configuration and eligibility

Publication configuration is a committed `.cargo/release_plan.toml` in the
selected Cargo workspace. A `--config` path selects another file; relative paths
resolve from that workspace and absolute overrides are accepted. Publication
requires tracked configuration inside the source repository. Configuration declares the intended GitHub repository,
release branch and binary target selection. Invocation-specific source identities
and output locations are command inputs, not persistent configuration.

The GitHub integration uses the same file through its `config` input rather than
maintaining a second set of per-field overrides. Package-specific target
restrictions belong under `[package.metadata.release-plan]` as `release-targets`.
The application defines the supported native targets; the workspace selects among
them, and package restrictions narrow that selection. Invalid targets and binary
packages with no selected supported target are configuration errors. Multiple
installable binaries are rejected, not silently reduced to one. Runner provisioning
belongs to the workflow, not to package-version assessment.

One default-feature executable is the canonical package artifact. Asset names
identify the package, version and native target, without a separate binary or
feature dimension, and the ZIP contains that executable at its root. This keeps
selection consistent with the package's `cargo-binstall` metadata and the shared
workflow. Supporting another executable or configurable feature selection requires
a corresponding unambiguous artifact identity and installation contract, not
merely relaxed metadata validation.

`prepare-publish` pins a clean source commit on the configured release branch's
first-parent history. It verifies the tracked Cargo inputs, package identities,
version groups, dependency requirements and released-content invariant against
that commit's own anchors. The required lockfile must be tracked and unchanged.
Preparation preserves it without resolving dependencies; Cargo enforces locked
resolution during publication and native builds. Publication does not repair the
lockfile. Ignored build output is not a source change.
Preparation repeats the merge gate's publication-input checks, including target
selection and the tag/archive URLs in `cargo-binstall` metadata, before any upload.

Adoption into an already-published repository includes an initial release-history
audit. Maintainers establish that each current published version corresponds to
the claimed source and that package/version identities belong to this repository.
Matching version strings or a newly imported Git history are not proof of matching
released content. Differences are reconciled through reviewed forward releases,
not by rewriting published versions or established tags. Once adopted, the release
process preserves that correspondence.

Every publishable workspace package at that snapshot contributes its exact
name/version request, including packages assessed as unchanged. Non-publishable
packages contribute no upload request. Preparation can read
remote availability, but neither existing tags nor the report's pending-release
subset substitutes for checking crates.io.

### Publication artifacts

`prepare-publish` creates the publication manifest once from the validated merged
source and its committed configuration. It discovers the workspace packages,
excludes non-publishable packages, and captures every remaining exact version
request. It also captures binary names, target selection and release-relevant
source identity needed to validate subsequent operations.

The manifest contains the destination repository and release branch,
repository-relative workspace and package locations, publication source commit,
effective configuration, package requests, and expected tag/archive identities.
It records the producing tool version and an explicit schema version. It contains
neither semantic increment decisions nor mutable "published" flags.

**The publication manifest does not change between phases or on retry.**
Consumers validate schema compatibility and the captured source/configuration
before remote writes; they do not reinterpret it against a moving checkout.
Changing the requested versions, source or effective configuration requires a new
preparation and a distinct manifest.

Each phase produces a separate outcome linked to the input manifest's identity.
`publish github` additionally produces derived platform batches after resolving
the actual tag targets. Each batch binds its parent manifest, package versions,
executables, target, release tags and peeled tag commits. The tag commits are
not backfilled into the original manifest: they are observations made during
GitHub reconciliation, possibly after the release branch has advanced.

Every frozen batch also has a content identity for its exact requested work.
Hosted outcomes record their workflow run and attempt independently of that
identity. Reporting selects the latest applicable outcomes while preserving older
outcomes; an older successful batch cannot satisfy a later reconciliation that
observed those assets missing, even when the requested work is identical.

Neither manifests nor batches contain credentials or runner-specific absolute
source paths. They remain transportable between jobs with different checkout
locations. A batch is frozen once emitted; rerunning GitHub reconciliation may
emit a new batch for the remaining work without editing an earlier one.

These conceptual sketches omit protocol envelopes, content identities and
ancillary fields; they are not literal input schemas:

```text
publication manifest P
  source: M
  repository: example/widgets
  release branch: main
  workspace: .
  effective configuration: selected native targets and publication settings
  requests:
    widget 1.5.0       -> tag widget-v1.5.0
    widget_impl 1.5.0  -> tag widget_impl-v1.5.0
    widget-cli 2.0.1   -> tag widget-cli-v2.0.1
      executable: widget
      targets: [x86_64-unknown-linux-gnu, x86_64-pc-windows-msvc]
      assets per target: widget-cli-v2.0.1-{target}.zip and matching .sha256

registry outcome for P
  widget 1.5.0: already present
  widget_impl 1.5.0: uploaded and available
  widget-cli 2.0.1: uploaded and available

GitHub outcome for P
  package tags: established at verified commits
  widget-cli release: present
  Linux asset pair: complete
  Windows asset pair: incomplete

Windows batch derived from P
  target: x86_64-pc-windows-msvc
  widget-cli 2.0.1, executable widget
  tag: widget-cli-v2.0.1
  peeled tag commit: N (verified release-equivalent descendant of M)
  requested assets: Windows ZIP and checksum

binary outcome for the Windows batch
  widget-cli 2.0.1: both assets uploaded and verified complete
```

The data flow is:

```text
merged source + configuration
  -> prepare-publish -> immutable manifest P
                         |-> publish registry -> registry outcome
                         |-> publish github   -> GitHub outcome + frozen batches
                                                              |
                                                publish binaries
                                                              |
                                                binary outcomes
```

Registry completion is a prerequisite for GitHub writes; the diagram shows
artifact consumption, not permission to run those phases concurrently.
A prior success receipt is diagnostic evidence, not permission to skip a fresh
completeness check. For example, a missing checksum after a previous successful
run still requires asset repair. Outcomes describe that attempt; remote state
determines what remains at the next attempt.

Missing or malformed required artifacts fail the phase rather than imply an empty
release. A valid empty work set is a successful no-op with an explanation.

### Registry publication

`publish registry` reconciles every requested exact version with crates.io.
Existing versions are not republished. Registry lookup failures are errors, not
evidence that a version is absent. A yanked version still occupies its version
identity; publication does not republish or unyank it. Presence alone does not
prove that a dependency is usable under Cargo's resolution rules. An unavailable
required dependency blocks its dependent's publication and prevents a complete
registry-phase outcome.

Cargo constructs and verifies the packages and publishes the selected missing
versions using its workspace-publication dependency semantics. Version-assessment
batches are not upload-order instructions: their development-dependency and
version-group relationships serve different purposes. Publication uses `--locked`
and leaves Cargo's package verification enabled. Cargo owns manifest and lockfile
normalization, archive hashing and upload. The application does not impose an
additional comparison between the normalized package closure and the workspace
closure used for pre-merge version assessment.

Completion requires observing the requested versions in the registry, including
availability needed by dependent publications. A Cargo failure after upload does
not prove that the upload failed. Before retrying, the application refreshes exact
version availability and omits completed uploads. Transient retries and
index-propagation waits are bounded; deterministic source or configuration errors
remain failures rather than being hidden by repeated attempts.

### Publishing identity

Automatic registry publication uses GitHub Actions OIDC and crates.io Trusted
Publishing. Short-lived credentials are acquired and renewed as needed for bounded
publication attempts and revoked when no longer needed. Authentication failures
do not select a stored-token fallback. Credentials never enter publication
artifacts, diagnostics or command-line arguments.

The consuming repository owns its Trusted Publisher registration and optional
protected environment. Registration names that repository and its calling entry
workflow filename, for both direct composite use and reusable-workflow use. The
job performing the exchange uses the registered environment. GitHub's
[reusable-workflow OIDC tokens](https://docs.github.com/en/actions/how-tos/secure-your-work/security-harden-deployments/oidc-with-reusable-workflows)
retain the caller's identity; the additional called-workflow claim is not a
replacement registration in the action repository. First publication of a crate
and publisher registration remain explicit maintainer setup; automatic
publication does not bypass that prerequisite.

GitHub writes use the job's ambient repository token. Registry publication,
GitHub reconciliation and binary publication retain separate permission scopes.
Scopes are job-level; limiting credential exposure to child processes is separate.
The application's binary-compilation subprocess environments omit registry and
GitHub upload credentials; its GitHub operations receive the upload token.

### Immutable tags and release-equivalent source

`publish github` confirms registry publication for the manifest's complete
package/version set before creating tags or releases. A successful registry phase
with no new uploads still runs this reconciliation. Missing requested versions
remain a failed prerequisite, not a smaller implicitly successful release.

The package tag is `{package}-v{version}`. Existing tags are authoritative and
never moved. For a missing tag, the application fetches and pins the configured
release branch, verifies that it descends from the publication source, and checks
the requested package version and its released content at that candidate.
Equivalence uses the same anchor-based content model as version assessment,
including inherited inputs and binary dependency closures.

A later equivalent snapshot is a valid tag target even when its unrelated files
differ from the registry-publication source. This permits new tags to use a
current branch snapshot under the ambient GitHub token without requiring
workflow-write credentials for historical workflow files. The write names the
verified commit, not a moving branch reference. If branch movement prevents the
write, a bounded retry verifies a fresh candidate without changing the requests.

An old request is not relabeled when the branch advances to a newer package
version. Automatic missing-tag recovery requires that the requested version remain
available at an eligible snapshot. Existing tags remain usable for binary repair
without selecting a replacement target or imposing new version policy on them.

This can occur during ordinary concurrent development, not only after a failed
run: another merge can advance the same package while its registry phase is still
running. Queuing release workflows does not serialize merges to the release
branch. If tag creation cannot complete after applicable bounded retries, the
application leaves that tag absent and records the release as failed. It skips
that release's tag-dependent work while continuing independent releases. The
reconciliation phase and overall workflow remain failed; skipping this work is
not a successful no-op.

The workflow posts a failure issue identifying the affected package/version, exact
missing tag, recorded publication source commit, reason for failure and original
workflow run. For a superseded version it also identifies the observed conflicting
branch version. Recovery instructions tell an operator to verify the source,
create the missing tag at that recorded commit using their greater access rights,
and retry the original failed workflow. They do not instruct the operator to tag
the latest branch tip or move an existing tag.

On retry, reconciliation refreshes remote tags and recognizes a manually created
tag through the ordinary existing-tag path. It validates the requested package
identity at the resolved target and does not reapply the current-candidate
requirement used only to create a missing tag. For a binary package it can then
create the GitHub release and emit the missing binary batches. Already published
crate versions and completed asset pairs are retained. Manual tagging clears this
tag-creation blocker; unrelated publication failures remain independently visible.

Libraries receive tags only. Binary GitHub releases are attached to their
established tags; release creation cannot choose another commit implicitly.
Binary builds use the actual peeled tag commit, not the version anchor, the
publication source by assumption, or the latest branch tip. Release equivalence
does not promise byte-identical binaries rebuilt in different environments.

### Native binaries and asset completeness

Binary discovery is package-driven and keeps the executable name distinct from
the package name. Each native target has one batch, with packages built separately
so their feature selections remain independent. Compatible Cargo artifacts are
reused across the batch. Each source snapshot supplies its tracked toolchain,
Cargo configuration and locked dependencies; installed automation remains
independent of the source it builds.

Each release publishes `{package}-v{version}-{target}.zip` and its matching
`.sha256` sidecar. The ZIP contains the executable at its root, with the target's
executable suffix and executable permissions where applicable. The package's
`cargo-binstall` metadata must describe this same layout.

Archive creation is included in the application; operators do not install a
separate ZIP utility. Packaging streams executable contents without requiring
memory proportional to executable size and remains subject to item cancellation.

The standard binary build uses Cargo's release profile and default features.
Publication configuration does not accept arbitrary Cargo arguments or silently
enable features to reach a binary. Package authors are responsible for making
their selected executable build under that selection. Cargo checks feature
requirements during the native build; the offline publication-input check does
not simulate feature resolution or compilation. The guide distinguishes this
build selection from the external library API checker's all-features comparison.

A release/target pair is complete only when both assets are uploaded. Execution
refreshes completeness before building, repairs both members of an incomplete
pair, and verifies upload completion afterward. Batch structure and frozen tag
identities are validated before execution; this preflight admits the complete
batch rather than a silently narrowed subset. After admission, independent package or target
failures do not suppress the remaining work, but any required failure makes the
run fail. Cancellation stops further work and publication while retaining
diagnostics for operations already attempted.

The archive and checksum are one integrity unit. A rebuild may produce different
bytes even from release-equivalent source, so a newly generated checksum must
not be paired with an unverified surviving remote archive. Repair replaces both
members using the locally generated pair.

The nonpublishing binary mode consumes an existing frozen batch and builds and
stages its requested archives without tag, release or asset queries or uploads. It
retains source and archive verification and requires no upload credential.
Repository-read access may still be needed to fetch a missing source commit. It
does not discover tags or select a pre-publication source on the caller's behalf.

### Outcomes and recovery

Each publication phase emits structured outcomes identifying completed, already
complete, blocked and failed work. Human summaries describe the same results,
including partial successes and incomplete asset pairs. Progress and failures go
to stderr; requested machine-readable outputs are not mixed with child-process
diagnostics. `--verbose` explains the inputs and reasons behind selection,
source verification, skips and retries.

A rerun retains the original requests and resumes from observed remote state.
A new push or manual recovery run can prepare its own manifest for the selected
release snapshot. Neither path depends on a list of packages uploaded in the
current attempt. A superseded version whose tag is missing follows the operator
handoff above; a new run against the latest source is not a substitute for retrying
the original version's publication.

Publication artifacts are handoff records, not a permanent release database.
The shared workflow preserves intent and per-attempt outcomes for its documented
artifact-retention period, including on failure. A failed-job rerun retrieves the
original manifest and any existing batch rather than rediscovering requests from
the current release-branch tip. After manual tagging, GitHub reconciliation can emit the previously
blocked batch from that original manifest. If required evidence is unavailable,
the rerun stops; an explicit recovery invocation
can prepare new evidence for a chosen retained source commit. That recovery
revalidates the source and remote state and is not a continuation of a missing
outcome. A removed source commit or incompatible artifact schema requires an
explicit diagnostic, never fallback to the latest source.

Registry publication and all GitHub phases execute within the same workflow run.
They do not depend on token-authored tags or releases triggering another workflow.
Publication does not roll back successful uploads or fabricate completion to
compensate for failed cleanup or reporting.

## Reusable GitHub integration

The integration exposes a command-selecting composite action and reusable
workflows over the same application. Consumers select committed configuration
and an immutable action revision. **Path installation** builds the controller
from an explicitly selected checkout; it tests application changes without
claiming that the corresponding package or archives are published.
The [implementation guide](implementation.md#reusable-action-boundary) owns the
action repository and bootstrap boundaries.

### Consumption and configuration

The composite and reusable workflows in `folo-rs/cargo-release-plan-action`
share one action release and tag stream.

The composite's required `command` selects version checking, compatibility
checking, preparation, registry publication, GitHub reconciliation, binary
publication or failure reporting.
Non-default command-specific inputs that do not apply to the selected command
are rejected. Shared installation settings retain their documented mode semantics.
Substantive selection,
validation, reconciliation and report composition belong to the installed application.

Reusable workflows provide the standard read-only merge check and release flow.
The check workflow resolves configured release history and the tested PR's target,
supplies publication configuration to `check`,
and performs scoped external compatibility checks. Repositories needing a narrower
version-readiness-only queue gate use the corresponding lower composite operation,
with the candidate's history and target explicit. The release workflow owns the preparation
and registry job, subsequent
GitHub reconciliation, native binary matrix, artifact handoff and failure
reporting. Consumers supply their triggers, required permissions, optional
publishing environment and configuration location.

Per-release tag failures do not suppress validated batches for other releases.
The workflow admits such batches only after registry prerequisites and their
manifest/batch validation succeed, even if reconciliation reports another release
as failed. Missing or invalid batch artifacts and cancellation do not grant that
admission. The final result retains the reconciliation failure and the reporter
posts its operator instructions; successful independent jobs cannot hide it.

The `working-directory` input selects the Cargo project; `config` uses the same
workspace-relative path rules as the CLI. Runtime inputs describe the invocation,
not another versioning policy or a scatter of configuration overrides.

The lower composite layer supports repositories that need a different job graph,
without bypassing the release invariant. Both layers use the same input names and
meanings; public list inputs use comma-separated values rather than caller-written
JSON. Structured internal matrices and publication artifacts are produced by Rust.
Reusable workflows invoke the composite from their own exact action-repository
commit, not an independently moving major tag or the consumer's local action path.
The bootstrap establishes that called-workflow identity separately from the
caller's source identity. It cannot require publication credentials merely to
resolve the action revision for a read-only check. Consumers can pin immutable
workflow commits for reproducible full reruns; documentation distinguishes those
from mutable major references.

The shared flow gates writes to the configured repository and release branch.
Read-only pull-request checks do not acquire publication credentials. The caller
grants the required scopes, and individual jobs narrow them; the reusable workflow
cannot add authority the caller did not grant.

Public repositories can run read-only checks on fork pull requests under the
repository's normal approval policy. They use the base repository's release
history and the tested PR source, not a fork's similarly named branch, and acquire
no write or OIDC authority. A policy-disallowed run is reported as not executed,
not as a successful release check. Privileged `pull_request_target` execution of
contributor code is not an integration requirement.

Publication runs for the same repository, release branch and workspace use
non-cancelling concurrency with `queue: max`. GitHub's
[queued concurrency](https://docs.github.com/en/actions/concepts/workflows-and-actions/concurrency)
retains pending invocations within the hosted queue capacity rather than replacing
them with newer pushes. Queue rejection or platform cancellation is not a completed
publication. Distinct platform batches run independently with matrix fail-fast
disabled. Artifact names and concurrency identities derive from that release
context rather than a caller-maintained instance identifier. Cross-job recovery
preserves the manifest and batch identities across job reruns and does not mistake
a missing artifact for no work.

Repositories needing native build prerequisites can provide
`.github/actions/release-plan-setup/action.yml`, using the same fixed-local-action
convention as benchmark setup. The shared workflow invokes it from the invocation
checkout before Cargo publication verification or binary builds. It prepares the
environment without changing the validated source, and does not run in GitHub-only
reconciliation jobs. There are no arbitrary hook paths or release-policy callbacks;
unusual job arrangements use the lower composite layer.

Failure reporting uses standard application-rendered summaries and a run-qualified
GitHub failure issue, independent of successful release artifacts. Reports remain
available for failed runs. Notification failures do not hide the original failure,
and inability to install the application does not invoke a second shell publisher.
Reporting does not require caller-created labels or message templates.

### Installation and version selection

The action's release manifest records its own version, the exact
`cargo-release-plan` version it selects, and the supported API compatibility
checker pin. Action and package versions are independent.
Consumers select a tested action revision, not a separately overridden tool version.
All phases of a released workflow use that selection.

That manifest also declares the supported native target/runner pairs. Workspace
configuration selects among those targets; it does not extend the shared runner
matrix. Repositories placing a supported target on another native runner use the
lower composite layer. The declared support set governs installation availability
and the action release gate.

The shared installation input names and behavior match the benchmark action:

| `install-method` | Behavior |
| --- | --- |
| `binstall` | Default: install the exact manifest version from published binaries, allowing Cargo source installation when no archive matches. |
| `install` | Install the exact crates.io version from its published source with its lockfile. |
| `path` | Build the application from the Folo checkout selected by `source-path`, using that checkout's lockfile. |

`path` does not substitute a released executable for the selected source.
Each operation installs only the application and external tools
it actually needs; publication-only jobs do not install compatibility checkers.
No separately configured publication helper is required. Installed execution has no dependency
on repository-root Folo scripts; its runtime prerequisites are supplied by the
shared setup.

The controller's supported Cargo/toolchain requirements are distinct from the
consumer workspace's build toolchain. Bootstrap selects the toolchain needed to
install the pinned application without inheriting an incompatible caller override.
Source publication and binary builds use their documented source-toolchain
contract. Unsupported combinations fail in preflight rather than first being
discovered after a registry upload.

Path installation can publish the application's own package without first
installing the version being published. It validates the chosen action revision
and source checkout together, not published-installation availability.

### Releasing the action

The action repository publishes immutable `vX.Y.Z` releases and a floating major
reference. The major advances only to an appropriately compatible tested action
release. A change to tool pins is an action release even when its YAML is unchanged.
The action's release manifest is the authority for its tool selection.

Tool changes that move a pinned version have a paired action change carrying the
new exact pin and an independently chosen action version. Tool publication
precedes action publication. The required installation gate verifies the exact
published application, promised archives and API compatibility checker.
It requires the executable identity and command contracts used by that action,
not merely a successful installation.

Missing package versions or archives block the action release. Source-mode tests,
cached executables and source fallback do not establish archive availability.
Retries preserve immutable version tags and never move the major reference
backward. The [implementation boundary](implementation.md#reusable-action-boundary)
describes distribution mechanics; the public guide owns consumer adoption and
upgrade instructions.

## User guide and reusable skill

The public book teaches the complete process without relying on these internal
design and implementation documents. It starts with motivation and goals, explains
version assessment and publication as separate responsibilities, and introduces
release history, merge targets, anchors, semantic impacts, plans and manifests before an integration
walkthrough. The walkthrough addresses the user's work: configure adoption,
invoke the skill, review its proposed release and merge under repository policy.
The workflows own successful delivery and its verification; the guide does not
assign a routine manual audit after every release.

Recovery explains how users discover failures through checks, workflow results
and failure issues, then choose between retrying, repairing prerequisites,
issuing a new recovery request and releasing corrected source. Advanced modes
and custom arrangements follow the ordinary path rather than obscuring it.

General release-process guidance belongs in that book. Repository instructions
select local policy and link to the book instead of maintaining another explanation
of anchors, compatibility levels or publication recovery. The package README is
an installation and quick-start entry point; CLI help and machine-format reference
provide precise interface details.

The `increment-versions` skill is a self-contained directory that a consumer can
copy into its repository. Its essential instructions and decision guidance travel
with it. Before preparation or edits, it checks the installed tool's identity
against its supported interface versions. Commands use the installed application
and documented prerequisites,
not Folo's Just recipes, sibling skills or root scripts. Repository and release
branch choices come from the selected workspace and explicit configuration, not
hardcoded Folo identities.

The book fixes the tool and skill interfaces used by its walkthrough and explains
how adopters select and record a compatible immutable action revision. It retains
access to revision-pinned instructions. Upgrading an action does not silently update a copied skill. A
consumer can verify compatibility before changing its repository or regenerating
local evidence.

The skill coordinates explicit compatibility evidence collection and publication
preflight as well as offline planning; those optional external operations do not
change the offline contract of `report` and `check`. It preserves captured
evidence, distinguishes operational failure from a compatibility finding, and
reassesses after relevant source or release-branch movement. Its output explains
the complete release set for PR review, including retained pending increments,
group alignment and first-publication handoffs.

Repository-specific follow-up, such as pairing releases with another action
repository, belongs in repository instructions outside the copied skill. Completion
does not authorize merging, first publication or release administration.

Internal ownership is documented in the [implementation guide](implementation.md).
