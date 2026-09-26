# cargo-release-plan implementation

User-visible behavior belongs in the package [design](design.md). This guide
describes the internal boundaries that keep that behavior consistent.

## Architecture

The binary and its internal library target are intentionally thin. `main` parses
Cargo's injected subcommand argument, then delegates to the library `run()` entry used by integration
tests. The library target re-exports only the required application/test wiring from
[`crp_impl`](../../crp_impl/docs/implementation.md), which owns the implementation,
unit tests, implementation-boundary integrations and benchmarks. Both packages
share an exact dependency and release version.
Both library targets are marked `private-api = true` and disable library
documentation; user-facing contracts are the CLI and documented artifacts.
The selected command drives command-specific paths through shared components:

```text
Cli -> RunInput -> run()
                    |
                    +-> classify -> check diagnostics
                    |           \-> report JSON + patches
                    |
                    +-> report artifact -> analysis batches / compatibility targets
                    |                 \-> semantic decisions -> proposed plan
                    |
                    +-> prepare -> offline workspace resolution -> evidence + input snapshot
                    |
                    +-> preview -> normalize plan -> disposable prospective workspace
                    |                            \-> rewrite + resolve + classify to fixed point
                    |                              -> expanded plan + captured files
                    |                              -> retained compatibility workspace
                    |
                    \-> apply -> validate original or fully applied snapshot -> install files
```

Modules own subjects rather than syntactic categories. `metadata` and `manifest`
build the work-tree model, `git` owns repository facts, `anchor` resolves release
history, `classify` combines those inputs, `groups` and `plan` expand release
decisions, and the command-specific modules own preparation, preview, application, and reporting.

Executable identity is handled by CLI parsing before workspace acquisition.
The implementation partition's compiled package version identifies the application
because their exact dependency keeps the release versions equal. Installation
checks do not need to inspect a consumer repository to identify the executable.

The publication subject validates committed policy independently of remote state.
Its configuration owns supported native targets but no runner assignments.
Package discovery shares the unresolved Cargo-metadata acquisition boundary with
classification, projecting the binary names, required features, registry
eligibility and archive metadata needed by publication. The optional configured
`check` composes this validation with ordinary version readiness; unconfigured
assessment performs no publication discovery.

Publication preparation composes that policy with the candidate verifier under
`publication::candidate`. The verifier owns clean-head/index checks, tracked
input containment, first-parent membership and candidate-relative version
validation. Its typed request comes from publication preparation, not another
executable or argument parser. Real candidate-boundary tests and the in-process
verification sequence belong to `crp_impl`.

The preparation boundary fetches the configured GitHub branch through a
per-invocation Git credential helper and captures its resolved commit. It does not
depend on a local remote nickname or modify global authentication configuration.
Full locked metadata verifies resolution without changing it. The publication
manifest then contains repository-relative paths and all exact package requests.
Canonical serde serialization supplies its SHA-256 content identity; schema,
paths, configuration and identities are revalidated on read and before writes.
Atomic no-clobber promotion prevents a different intent from replacing an existing
handoff. Outcomes are not part of that identity and belong to separate artifacts.

Artifact-only planning shares the report producer's serde model. Report loading
validates the schema and cross-package identities before consumers build dependency
graphs or version targets. Analysis ordering follows recorded dependencies rather
than exact-version grouping. Compatibility selection follows group closure but
emits only packages declaring a consumer contract.
Dependency and dependent references must name a reported workspace target; an
incomplete report cannot silently remove a relationship from release assessment.
Classification and report validation share the status derivation from anchor,
declared version, and change evidence. Deserialization cannot manufacture a pending
release without a version increase or comparison evidence for an anchorless package.

Report group validation separates member ordering from uniqueness. Ordinary sortedness
checks the order, while one membership set rejects repeated members within or across
groups. Together these enforce strictly increasing members without a redundant strict
comparison. Shape tests keep package references reciprocal while independently varying
group size, canonical naming, ordering and uniqueness.

## External compatibility evidence

Compatibility is a separate explicit operation, not part of offline classification.
Prepared inputs and resolved previews already carry source identity, so the checker
consumes those artifacts rather than extending report schema solely to make an
unbound report executable. Fresh checks capture inputs around their own read-only
report and verify them again after external comparison. Preview checks use the
retained prospective manifest and existing resolved-state verification.

The checker receives all features and one explicit published baseline version per
consumer contract. A small identical-source library canary validates its ability
to run. A short workspace-identity-keyed target directory avoids generated Windows path
length problems while retaining compatible compiler artifacts across assessment
passes, without affecting other Cargo commands. Checker findings and
execution errors remain distinct; a completed comparison supplies a semantic
floor, not the author's compatibility judgment. Empty target sets do not invoke
external tooling or query registry versions.

Registry preflight reuses exact registry observations and resolved-plan inspection.
The plan-scoped form fails closed for unavailable or never-published targets;
workspace discovery remains an explicit advisory. Neither form performs package
administration.

## Registry publication boundaries

Registry observations distinguish an absent exact version from an unavailable
query. Cargo owns workspace packaging, verification and dependency-ordered
uploads; the application selects only missing requests and rechecks availability
after the attempt. Outcomes preserve partial completion and distinguish a dry-run
plan from confirmed delivery. Each attempt writes a new outcome file rather than
overwriting intent or earlier receipts.

The Cargo credential provider acquires a fresh GitHub assertion and crates.io
token per upload, after Cargo package verification. It checks the requested
registry/package/version and the binary archive's Cargo-supplied checksum before
releasing a credential. Normalized workspace registry identities are compared
with the source's installation closure using the existing lockfile model.
Packaging may prune inactive feature branches, but cannot select an identity
outside that assessed closure. Cargo's compilation still verifies the package.

Cargo starts short-lived provider processes. Invocation-owned temporary files
therefore retain identity context and issued token leases for the parent to revoke
after Cargo exits. They live outside source and publication artifact directories,
are never transported between jobs, and are removed when the attempt completes.
This avoids a separate credential broker service or token-renewal scheduler.
Registry and GitHub credential variables are removed from Cargo's environment;
the provider receives only its private context location. This is credential
handling within the trusted publication job, not process isolation from reviewed
build code running under the same account.

OIDC HTTP errors report the operation and status without echoing response bodies,
and credential values have no diagnostic representation. Revocation and temporary
directory failures remain failed outcomes even when uploads succeeded. The
registry build directory is independently owned, so packaging cannot dirty a
source checkout merely because that repository has no target-directory ignore.

## Native binary execution

`publication::binaries` owns the native batch engine behind `publish binaries`.
Frozen manifest-linked batches are its only job protocol; runner assignments
and workflow timeouts belong to the shared action. Batch decisions and
source/artifact validation stay in the implementation partition's unit tests.

The controller repository supplies Git objects and the shared target directory,
while each release tag selects a disposable immutable source worktree.
Missing objects are fetched by exact commit from the configured GitHub repository;
the caller need not have a local remote named `origin`. Fetch authentication is
scoped to that repository-read command, not inherited by compilation.
The controller workspace's repository-relative location is retained for nested
Cargo projects. Build commands execute there and rustup selects a tracked
toolchain within that source repository; the engine needs no repository-local
PowerShell toolchain adapter. Source preparation verifies the compiler's actual
native host before installing its target.

Owned process groups implement cancellation and deadlines. Build environments
exclude upload credentials and controller toolchain overrides. Independent
packages retain separate Cargo invocations, while compatible target artifacts
are shared. The batch retains source cleanup diagnostics alongside publication
outcomes, including both Git-worktree and directory cleanup failures.
The GitHub asset adapter owns its executable and token separately. Integration
tests supply a native protocol fixture without altering process-global
environment or introducing a second release-command implementation.

## GitHub reconciliation and reporting

GitHub reconciliation checks the complete registry prerequisite before writing
tags. The source-candidate classification is shared across missing-tag requests
and compares the fetched first-parent descendant against the original publication
source's version anchors. Per-package eligibility requires unchanged released
content and the exact requested version. Bounded creation retries refresh that
candidate; a version superseded before tagging produces the explicit operator
handoff rather than another version or broader credentials.

Existing tags retain their commit identity and bypass candidate selection.
Historical package identity is checked without imposing today's configuration or
group policy on an old tag. Reconciliation carries that verified commit into
release requests and batches without replacing it with another tag observation.
Binary releases name the established tag explicitly,
and creation requests retain its observed commit instead of an implicit branch
target. A competing ref created at a different commit is preserved but does not
authorize this attempt's release or binary work.
Paginated asset inventories determine which native pairs remain incomplete.
Tag failures retain per-package diagnostics and do not discard valid batches for
other releases. Batch identity hashes the complete native request set, including
tag source commits, independently of a workflow attempt.

The REST client owns structured tag/release/issue operations; the native engine
retains GitHub CLI asset upload so it can use its existing process supervision and
archive-file interface. Both use the invocation's repository token. Git supplies
source objects independently of either forge API boundary.

Outcome artifacts add optional GitHub run/attempt attribution; publication intent
does not. The final reporter selects the latest applicable phase receipt for the
current publication/run, then matches binary receipts to the batches named by that
GitHub outcome. A binary receipt cannot precede the reconciliation attempt that
observed the missing assets, even if the batch identity happens to match.
Platform job failures and cancellation remain authoritative over old receipts.
Missing manifests or receipts produce an incomplete report and operator issue,
never reconstructed intent from the current branch. Invalid or ambiguous receipt
evidence does not discard the independently acquired platform job failures.

`release-context` gives both local planning and shared workflows the same
configured baseline and concurrency identity. It is read-only apart from fetching
Git history and does not require clean source. The separate identity-setup probe
exchanges and immediately revokes OIDC credentials without publishing, allowing
caller/workflow registration to be verified before a live release.

## Subprocess boundaries

All repository access goes through `GitRepo`, which spawns the installed `git`.
The tool therefore follows the repository formats, filters, configuration, and
extensions the maintainer's Git understands instead of maintaining a second Git
implementation.

The subprocess boundary also covers `cargo metadata --no-deps`,
`cargo package --list`, and explicit offline preparation/preview. Classification never
runs a build or a full dependency resolution.

Git paths remain repository-relative and `/`-separated. Operating-system paths
from Cargo are converted once when they enter the manifest model. NUL-delimited
Git output is used for file names and decoded strictly as UTF-8; substituting an
invalid byte could collapse two different paths into one. Other command output
is decoded lossily because replacement there affects only diagnostics.

Every subprocess receives a fixed locale and disabled Cargo color. One routine
Git failure must be interpreted in process: a path absent from a revision. A
translated diagnostic would otherwise turn ordinary package creation or deletion
into an error.

Output capture and nonzero exits belong to integration coverage, including
repository-independent Git operations in disposable directories. Tests that need
repository state use explicit hermetic integration fixtures. The source tree's
Git metadata is never a test prerequisite.

Cargo subprocess arguments retain the required `Cargo.toml` basename even when
canonicalized captured inputs record another spelling. This conversion follows
the containing directory's probed alias behavior; it never redirects a distinct
manifest on a sensitive filesystem. Git lookups continue to use recorded spelling.

### Test boundaries

`cargo-release-plan/tests/integration/native_binaries/` drives the unified
executable with publication manifests and sealed batches. It covers historical
and nested sources, exact missing-source acquisition, independent package
failures and feature selection, shared build output, native archive permissions
and checksums, rejected artifact paths, and process-tree cancellation. The native
GitHub upload boundary runs in `crp_impl/tests/boundaries/native_binaries.rs`,
including incomplete uploads, retries and source cleanup failure.
`just release-binary-smoke` selects both owners on the native platform.
Ordinary test and coverage selection includes both targets.

Candidate-boundary tests in `crp_impl/tests/boundaries/candidate/` cover actual
Git index, history and tracked-input semantics and real Cargo verification.
Windows candidate cases share a nextest group and an in-process libtest slot
before starting their watchdog, so queued cases do not consume that budget.
Other platforms retain normal parallelism.
The candidate watchdog is a last-chance native-process guard, accommodating
instrumented Git/Cargo startup without making elapsed time a test assertion.

Registry-publication boundary tests invoke real Cargo against an isolated sparse
registry. The fixture retains uploaded archives and immediately exposes their
index entries, so ordering and package verification exercise Cargo's own behavior
without production registry access. Archive inspection checks normalized dependency
identity and preservation of the source lockfile. The HTTP fixture explicitly uses
HTTP/1.1; it does not implement cleartext HTTP/2 upgrades.

The same fixture runs a standalone credential provider that records Cargo's
requests alongside package build-script events. Uncached, operation-specific
credentials are requested separately for each upload after verification. This
keeps the token-acquisition boundary aligned with the upload rather than the
potentially long compilation phase. Partial publication is exercised by uploading
the dependency first and publishing only the remaining dependent afterward.

The registry runtime boundary supplies credential sessions, Cargo process results
and retry delays. The CLI binds it to native operations. Additional integration
tests retain real Git/source checks and loopback registry observations while
controlling process completion, covering partial uploads, lost success responses
and changed source without publishing packages. This is an internal testing
boundary, not a selectable registry or publication backend.

The [workspace in-process boundary](../../../docs/testing.md#unit-tests-stay-inside-the-process)
applies to every fixture and acquisition call. Avoiding Cargo metadata or keeping
a real Git history small does not make an acquisition test a unit test. Tests of
real Git, Cargo and filesystem adapters belong in Cargo integration targets;
decision tests supply acquired values without calling those adapters.

`crp_impl/tests/boundaries/` preserves direct assertions on acquisition, files,
repository state and private error conditions. Its Git fixture cannot be imported
by library unit tests. The unit-only Git helpers construct inert handles and
tree entries without observing the host.

The executable-connected `cargo-release-plan/tests/integration/` suite stays in
the binary's package: Cargo supplies `CARGO_BIN_EXE_cargo-release-plan` only to that
package's integration targets. This is the executable-ownership exception to the
usual implementation-crate test layout, not a second implementation or nested
build harness. The shell also checks its internal re-export boundary.

Captured-input decisions use acquired metadata and a read-only per-directory case
probe. Unit tests supply regular-file, missing-file and error observations, mixed
directory case rules, and exact candidate/replacement bytes. They verify the
persisted fingerprint framing, Unix execute-bit interpretation, alias collapse
without new input admission, and retained verification ordering independently of
the host filesystem. Windows path-prefix conversion and Unix mode interpretation
are compiled for all test hosts because those transformations are pure.

Released dependency discovery resolves Cargo's dependency paths against the same
canonical member index as exact-version grouping. Equivalent member and dependency
spellings, including Windows verbatim paths, retain the same graph in ordinary,
prepared and fresh compatibility reports. Filesystem resolution stays in metadata
acquisition; membership decisions use acquired observations in unit tests.

Offline resolver invocation and changed-artifact selection have in-process cores
that preserve arguments, working directories, bytes and errors. Preparation and
preview retain integration-owned Git/Cargo workspace orchestration and completion
output. Convergence receives a digest operation alongside the resolver pass:
production uses Git hashing, while unit tests provide in-process state identities.
Their decision helpers, changed-write selection and convergence loop remain
mutation targets. Read-only verification separately tests live-input validation
before retained-candidate validation. Only the corresponding real acquisition and
process adapters are excluded from mutation discovery.

Writable fixtures derive their paths and recursive cleanup roots from their own
temporary-directory owners. Production canonicalization and emitted artifact
paths are observations to assert, never authority for fixture writes or cleanup.
This remains true when an assertion unwinds or a path helper is mutated.

Pure decision and validation tests own the combinations of versions, dependency
forms, captured-state differences, and artifact selections. They use small inputs
without acquiring repository state or resolving a Cargo workspace. Orchestration
tests inject acquisition at its existing boundary so they exercise production
ordering, including rejection before writes and completion-marker invalidation,
without rebuilding a successful preview for every failure case.

Proposal and expansion inject artifact operations into their command cores.
Their unit tests retain input-collision checks, acquisition and publication order,
plan generation, rendered output, and error propagation without touching files.
Proposal publication also checks stale-output invalidation and failed-write cleanup.
Real artifact path interpretation and file access stay in integration coverage.

Anchored classification converts acquired evidence into a verdict through an
in-process boundary that owns version-regression rejection and evidence retention.
Proposal tests observe exact alignment outcomes for moving members and members
retaining published versions. Final group validation is exercised independently
of normalization, including non-publishable targets, so its plain/equal-version
invariant remains observable even when normal generation already ensures it.
Semantic decision notes use the shared diagnostic sink to expose their decision
level and version inputs without capturing process-global stderr.

Report and check share synthetic classification snapshots with inert repository
handles; their in-process tests never acquire Git, Cargo or filesystem state.
The check core owns classification failure propagation, diagnostic-derived
verdicts and optional packaging coordination. Packaging observations remain
non-gating regardless of the release verdict.

Report publication separates artifact construction and sequencing from the
filesystem adapter. The core selects publishable assessments and nonpublished
version targets, emits the selected patches verbatim, and completes only after
all patches succeed. The adapter owns invalidating the previous marker before
replacing the patch tree and staging JSON beside its destination before promotion.
Integration tests retain replacement and failed-staging coverage; library tests
observe complete report contents, patch selection, publication order and errors.
Only the real classification/publication adapters are excluded from mutation
discovery.

Analysis-order and semver-target command cores acquire a validated report through
an injected loader and serialize the existing selection algorithms' results.
Small unit fixtures connect acquisition and errors to actual JSON output rather
than duplicating graph and group-selection matrices. The filesystem adapters
retain integration coverage for path forms, malformed reports, read-only artifact
consumption and execution without workspace discovery.

Expanded-plan inspection injects captured-state validation, retained-candidate
verification and workspace acquisition into the shared in-process orchestration.
Unit tests observe validation selection and ordering, propagated failures, the
intersection of publishable and selected targets, and complete serialized output.
Only the thin adapter that reads the artifact and connects real Git/filesystem
operations is excluded from mutation discovery. Integration coverage verifies
that inspection uses read-only application validation and rejects stale live or
retained workspaces without resolving Cargo again.

Manifest application injects captured-plan application, workspace acquisition,
manifest reads and writes into its shared orchestration. Unit tests exercise
dispatch, full edit computation before writes, unique root/member acquisition,
package and dependency rewrites, dry-run output, and unchanged-write suppression.
Prospective resolution uses the same edit computation. Only the adapters that
connect these operations to real files and repository state are excluded from
mutation discovery; integration tests retain the actual application boundary.

Integration tests establish the real Git, Cargo, filesystem, and executable
connections: history and index semantics, manifest discovery, offline resolution,
captured-workspace identity, and a complete CLI release-plan round trip. A test
classifies each unchanged workspace state only once where the resulting report
can establish all its assertions. Output-format combinations belong to renderer
tests, not additional repository classifications. Structural expansion tests stop
at the expanded artifact; only preview tests acquire resolved evidence.

Fixtures remain independently mutable. Immutable Git initialization and empty
global configuration can be shared within a test process, while commits still
use Git's normal index and filtering behavior. Process-local reuse must not be
assumed to span nextest's separate test processes. Both native and mutation runs
use Cargo's test classifications: ordinary testing includes integration targets,
while mutation testing selects only library unit-test targets under the
[workspace policy](../../../docs/testing.md#mutation-testing-target-selection).

History acquisition belongs in integration tests using small hermetic Git histories
without loading Cargo metadata. These verify first-parent ordering, manifest selection and endpoint
retention directly. Local shallow fetches exercise missing-parent evidence and
preserve the distinction between a truncated branch and a true root in the same
repository; commit messages remain separate from parent headers.

Installation acquisition belongs in integration tests using small temporary repositories without
Cargo metadata or resolution. They distinguish historical blobs from work-tree
files, check ancestor and filename configuration precedence, and retain missing
versus unreadable-input behavior. Pure source-comparison and patch-applicability
tests cover the decisions independently of acquisition.

Released-file discovery acquisition belongs in integration tests using small Git indexes and filesystem fixtures
without constructing or resolving Cargo workspaces. They exercise selection,
presence, modes and cleaned blob bytes at the acquisition boundary. Optional
reads and hash-input validation inject metadata and byte-read observations so
disappearance between operations, permission failures and symlink rejection remain
deterministic without races, delays or host symlink privileges. Real-filesystem
link tests also exercise the metadata adapter on platforms that permit them.

Filesystem path tests create symlinked temporary roots explicitly rather than
depending on the host's temporary-directory layout. Expected destinations use a
canonical existing ancestor followed by the missing suffix, preserving assertions
about symlink resolution and parent traversal without assuming a root spelling.
Artifact path resolution accepts injected canonicalization and directory queries
for deterministic operational-error tests. A transient failure must propagate even
if a subsequent query would succeed; tests do not depend on filesystem races or
the host account's permissions. Output staging is tested before promotion so its
same-directory placement, complete contents, and unchanged destination are observable.

## Workspace snapshots

`cargo metadata --no-deps` supplies candidate current members and normalized
dependency relationships. Git-tracked manifests constrain that candidate set.
The current model keeps both every tracked version target and the publishable
`WorkPackage` projection used for classification. An untracked or ignored
manifest found through a member glob can become neither a version target nor a
release assessment. Each tracked current member manifest is loaded and parsed
once per work-tree snapshot; its parsed document and derived package facts are
shared by version-target construction, exact-dependency discovery, and the
publishable projection. Historical workspaces cannot use Cargo without checking
out each commit, so `SnapshotCache` reconstructs them from tracked manifests.

The reconstruction starts from the root package and declared member patterns,
then follows in-workspace path dependencies to a fixed point while honoring
`exclude`. Explicit members may use `[package] workspace` from elsewhere in the
same Git repository; their workspace-relative parent components are retained
while Git access remains repository-relative. A non-virtual root is always a
member. Parsed manifests are cached per commit because anchor resolution and
content comparison revisit the same snapshots across packages.

Each snapshot resolves the package fields that may inherit from
`[workspace.package]` and the path dependencies inherited through
`[workspace.dependencies]`. Typed TOML values are retained for comparisons so
formatting changes do not masquerade as inherited-value changes. Dependency
table kinds are retained so versionless dev dependencies omitted by Cargo do not
create false inherited-value changes.

Exact dependencies are discovered from effective raw declarations, not Cargo's
normalized requirements. Both package identity and resolved member directory
must match. Parsing the entire suffix after `=` as a SemVer version enforces a
complete triplet and rejects compound requirements; separate prerelease and
build-metadata checks retain the plain-release-only rule.

Dependency unit tests use synthetic metadata and parsed manifests for declaration
and exposure decisions. Tracked-member selection uses an inert repository handle
and recorded paths, without initializing Git. Real Git and historical path
acquisition belong in integration fixtures. Canonical fallback integration tests
use equivalent filesystem paths without requiring symlink privileges.
Exposure tests retain propagation through private intermediaries, revisit
earlier dependents until closure settles, and distinguish normal edges from
build and development edges at every hop.

## Classification

Classification combines one current work-tree model with package-specific
historical snapshots:

```text
baseline first-parent commits
        |
        +-> package timeline -> anchor snapshot
                                      |
work-tree metadata -------------------+-> released-content comparison
                                      +-> inherited-value comparison
workspace lockfiles ------------------+-> installable binary closure comparison
```

### Anchor and change set

Only first-parent commits that can affect a parsed manifest are reconstructed,
plus the newest and oldest commits needed to distinguish a true root from a
shallow boundary. `anchor` walks the resulting package timeline until it finds a
version change, package creation, or insufficient history.

A package not published by the baseline bypasses the walk and becomes new.
Within a walk, an unpublished manifest is skipped rather than treated as absent:
withdrawal releases nothing, but an earlier published version can still be the
anchor.

After resolving the anchor, each side independently supplies the package
directory, packaging rules, resources, nested-package boundaries, and default
README. This keeps moves and changes to packaging structure comparable.

### Content identity and file modes

File equality uses Git object ids. `git hash-object` applies the same clean
filters and line-ending conversion used when committing, so work-tree bytes are
not compared directly with already-filtered historical blobs. The cleaned
work-tree blobs are written into Git's object database and read back by object id
when a patch needs their bytes. This keeps verdict and presentation on the exact
same filter result without changing refs, the index, or work-tree files;
unreachable blobs remain subject to ordinary Git garbage collection.

Historical file modes come from `git ls-tree`. Work-tree modes start from the
index and overlay `git diff-files --raw`: on a checkout with `core.fileMode`
enabled, an unstaged executable-bit change is observed; when it is disabled, the
index remains the stable fallback. Indexed symlink modes are retained as well
because `core.symlinks = false` can materialize a link as an ordinary work-tree
file; released symlinks are rejected before content hashing.

File bytes are loaded only after object identity differs, because mode-only
changes render directly and bytes are needed for presentation rather than the
verdict. Symbolic links are rejected before hashing or reading their targets.

### Package boundaries and resources

Tracked nested manifests define package boundaries independently of workspace
membership. The current side narrows that set to paths still present on disk, so
a deleted nested manifest no longer excludes its former subtree.

Manifest-named resources are resolved separately because they may live outside
the package directory and bypass `include` and `exclude`. Resources outside the
package are flattened to the archive root, matching Cargo's layout. Their
tracked state is queried explicitly so an untracked external README cannot affect
a verdict.

Path case is probed at the workspace root and reused throughout each current and
historical snapshot. Relative-path matching compares whole components and retains
Git's recorded suffix. Historical manifests, configuration and lockfiles resolve
to recorded tree paths before blob lookup. Implicit path members use recorded
directory keys, so dependency aliases cannot omit members or introduce duplicate
membership. The manifest-history pathspec follows the same probed rules.

The read-only filesystem probe forwards directory entries and case-flipped entry
checks, without following symbolic links, to a pure decision function. Unit tests
exercise both possible filesystem responses and ambiguous entries independently of the host volume;
real-filesystem regressions verify the acquisition boundary. An inconclusive probe
chooses case-sensitive matching, which does not widen the selected content.
Insensitive historical selection uses a full Git tree listing: Git can record
files under differently cased directory prefixes that merge in the checkout,
and `ls-tree` cannot express case-insensitive pathspecs. Final packaging and
resource matching determine the released files, not the breadth of the listing.
Cargo's own reserved packaging names and include/exclude patterns retain their
literal matching semantics.

### Patch rendering

`diff` implements a bounded Myers line comparison. Its working set follows the
edit distance rather than total file size, and it falls back to a whole-file
replacement after the budget is exhausted. The fallback remains a correct patch
and cannot change the verdict, which was already established from object ids and
modes.

The search preserves the furthest candidate after taking each edit, not merely
the furthest predecessor. Deletion advances the old-line position and insertion
does not, so equal predecessor positions require deletion. Forward search and
backtracking use the same choice. Small overlapping-line fixtures verify valid
line consumption, minimal edits within the budget, and valid replacement below
that budget without selecting a preferred spelling among equivalent scripts.

The renderer carries a file's content and mode together so an absent side cannot
accidentally receive a mode. Binary files receive presence and mode headers but
no textual hunk.

### Lockfile closures

`lockfile` parses only package identities and dependency references. Each
package with an installable binary target starts a breadth-first walk from the
source-less entry matching both its name and declared version. Name alone is
insufficient because another path dependency can share it; missing source alone
is insufficient because that dependency can also be source-less.

Dependency references are matched by every component Cargo writes: name, then
version and source when present. Parsing indexes these components and resolves
each textual edge to exactly one entry index once, so closure walks do not
search the package list. An unresolved or ambiguous edge makes the lockfile
incomplete and stops classification. A visited set terminates cycles. The root
is excluded from the result even if a dependency cycle reaches it.

The parsed work-tree lockfile is shared across all binary packages.
Historical lockfiles are shared by packages with the same anchor commit, so a
workspace-sized endpoint is parsed once rather than once per package.

An endpoint-specific installation graph supplies effective normal and build
declarations for every tracked workspace member, including unpublished members.
At each matching source-less lockfile node, outgoing edges must satisfy the
declaration's package name, version requirement, and source identity. Checking
only name and version would admit a same-named development dependency from a
different source. Applicable patches and named-registry configuration participate
in that source comparison. Direct path declarations and path patches retain their
manifests' exact package names and versions, including excluded targets. A broad
compatible requirement cannot distinguish separate source-less packages.

An unavailable installation-only path identity is required only when a binary
closure actually reaches that declaration. Unrelated library assessment does not
depend on reconstructing install-time resolution.

Source comparison accepts identical URLs directly and otherwise supports Cargo's
ordinary ASCII hierarchical spellings, including trailing-slash, `.git`, and
GitHub normalization. Missing registry mappings, unknown protocols, and unequal
URLs needing unsupported normalization produce operational errors rather than
silently excluding an installation dependency.

The dependency-table selector is shared with manifest rewriting. Canonical
hyphenated table names take precedence, including when empty; otherwise supported
legacy underscore spellings participate. Reading and rewriting therefore agree
about which declaration Cargo uses.

Both endpoint target shapes come from explicit manifest target declarations and
Cargo's automatic binary layouts, while respecting the manifest's `autobins`
control. Work-tree automatic discovery considers only tracked
paths that remain present, so an untracked or ignored source file cannot turn a
library artifact into an installable binary artifact.

Each endpoint that has an installable binary target must have a lockfile resolving
the package at its corresponding declared version. An endpoint without such a
target contributes an empty closure and does not require a lockfile. Endpoint
selection belongs to closure comparison itself, so classification needs no
separate binary-target gate. Missing or
incomplete required lockfile data stops classification because regenerating
historical resolution would violate the offline, no-full-resolution boundary. A
package absent from the baseline returns as new before lockfile comparison
because it has no historical artifact.

## Check and report

`check` and `report` consume the same `Classification`; neither recomputes release
rules. `check` renders failing package and group verdicts in text and optionally
as escaped GitHub workflow commands. Its packaging probe compares Cargo's list
with the exact work-tree selection produced by classification.

`check`'s verdict is read back from the rendered diagnostics rather than
recomputed from the classification, because every gating rule already appends a
line. A rule added to the rendering therefore cannot be reported without also
failing the check, which a second condition kept in step by hand would allow.

Some rules are properties of the manifests rather than of the released-content
comparison. An intra-workspace requirement must name the version its target
declares, which is checked against the normalized requirement `cargo metadata`
reports, so a bare requirement arrives as a caret one and both spellings that
name the version are accepted. A package whose public API exposes another
package must move incompatibly whenever that package does, compared against each
package's own anchor so an increment that landed in an earlier pull request
still counts.

Version-group discovery reads effective declarations from every tracked member
before the publishable projection is built. It resolves local path identity,
dependency aliases, workspace inheritance, dependency kind, and target-specific
tables without a full Cargo resolution. Exact requirements are validated from
the raw effective TOML because Cargo normalization loses suffix and compound
syntax. Validated declarations retain their source and target identities for
stale-version diagnostics. The resulting undirected edges are reduced to
deterministic connected components independently of release classification.

`apply` preserves each requirement's exact-or-compatible spelling while
rewriting the version it names, so applying a plan maintains both forms rather
than having to re-derive them. A hand-written requirement of the wrong form is
therefore caught by `check` rather than silently corrected, which is the right
split: it is a manifest edit, not a release decision.

Which dependencies are public is read in `metadata` from each package's
`allowed_external_types` allow-list, whose leading path segments name crates.
Matching follows `wildmatch`, the pattern language cargo-check-external-types
itself uses, against library target names taken from the target rather than
derived from the package name, so a `[lib] name` override cannot silently break
it.

An allow-list names the crate defining a type, while the release decision needs
the direct dependency supplying it. The two are bridged by growing each
package's exposed set to a fixed point: a dependency edge is public when what
that dependency exposes intersects what this package names, and its exposed set
then joins this package's own. The sets only grow and are bounded by the
workspace, so this settles; the bound is asserted rather than assumed.

`report` serializes the full package and group assessment, then writes patches
only where file differences exist. It removes any earlier `report.json` marker
before replacing the patch tree and writes the new marker through a same-directory
staging file after every patch succeeds. A failed rerun therefore cannot present
stale JSON and a partial patch set as one complete assessment.

## Plan resolution and application

Proposal generation turns caller-supplied semantic change decisions into ordinary
plan increments. It uses the shared group and plan resolver for version algebra
and target expansion rather than reproducing those rules in workflow scripts.
Group realignment and dependent release propagation settle together before final
plan invariants are checked. Pure Rust scenario tests exercise the generated
outcome, including no regression, complete group alignment, and release coverage
for rewritten dependent requirements.

The repository's PowerShell boundary invokes artifact commands and handles
compatibility subprocesses and registry publication probes. Registry availability
is not evidence for the Rust tool's offline release decision.
Expanded-plan inspection uses the shared target resolver and application dry-run
validation to return publication-eligible names and the evidence manifest. Workflow
adapters therefore do not maintain another plan-schema validator or rediscover
publication eligibility from package naming.
Inspection and compatibility verification share the candidate-location and
captured-state checks, so metadata cannot direct a caller to an unchecked workspace.
Proposal and preview output guards resolve existing path ancestors before
normalizing missing components. Creating an output directory therefore cannot
turn an accepted destination into an alias of the input evidence.
The PowerShell preview wrapper leaves initial directory creation and marker
invalidation to Rust. It may invalidate a successfully produced preview if later
compatibility evidence fails, but a rejected native invocation grants no ownership
over the requested output path.
The expansion wrapper requests Rust's input-preserving mode and supplies the final
destination directly. Rust checks aliases and promotes an exclusively created
temporary file only after a complete write. General expansion retains its separate
in-place behavior when that mode is not selected.

`plan` owns both planning stages and the resolution shared between them. It first
resolves package and group entries into one target version per tracked version
target. Levels combine by taking the highest and matching explicit versions
coalesce. Mixed decision kinds, conflicting explicit versions, regressions, and
non-plain group targets fail before writes.

A plan's stage decides what resolution guarantees. A proposed plan may reach
packages it does not name, which is how a decision about one group member moves
the group. An expanded plan must resolve to exactly the set it names and must
already carry a version for each, because that document is what a caller
recorded; reaching another package means the exact dependency graph changed after it
was written, and a surviving increment level would be re-resolved against the
manifests of the day. Both are rejected. The stage is matched on rather than
tested as a condition, so a new code path has to state which rule it wants.

Structural expansion and prospective preview share version-target resolution.
Both read the same Git-tracked version-target set. Preview also computes manifest
edits and resolves the resulting lockfile before writing the complete artifact.

`apply` accepts plan targets and validates groups against the Git-tracked
version-target set. It parses and rewrites every affected
manifest in memory before writing any of them. All Cargo-visible members remain
rewrite candidates, including non-publishable, untracked, and ignored members,
because they may carry exact pins to a package being incremented. A dependency
requirement is changed only when:

* the entry has a path,
* that path resolves to the named workspace member,
* the resolved versions include the member, and
* the existing requirement does not already name the new version.

The last criterion is the same predicate `check` validates the requirement
convention with, kept in one place so the two cannot drift: `apply` must rewrite
exactly what `check` would reject, and leave exactly what it would accept.
Leaving an already-correct requirement byte for byte keeps an alignment, which
resolves the leading member to the version it already declares, from editing
that member's dependents under an unchanged version.

Paths are normalized lexically first and canonicalized only for link or
case-variant spellings, keeping the ordinary path free of filesystem calls.

### Prepared and prospective resolution

Preparation owns the intended `cargo update --offline --workspace` refresh before
the report is graded. A workspace-wide update avoids ambiguous bare package names
when the lockfile also contains a registry package of the same name. Offline
resolution may reselect transitive edges among versions already locked; it is not
limited to replacing workspace package identities.

The prepared artifact contains only the post-refresh input snapshot. It cannot
carry an alternative file overlay: both semantic grading and prospective cloning
must start from the live state that preparation captured.

Preview uses a disposable prospective workspace so version and requirement
rewrites never become original planning inputs. Each iteration derives its
candidate from the prepared input rather than incrementing the preceding
candidate again. Classification uses the same pinned release baseline throughout.
New binary closure effects and their dependent/group consequences expand the
candidate until it is stable. Existing sufficient versions are retained.

The convergence loop is separate from the callback that rewrites manifests,
resolves offline, classifies and captures each pass. It returns only when both
version consequences and captured file contents are unchanged. Library tests
drive successive resolver outputs through this same loop, including changing
files with unchanged version decisions, before any final evidence verification.
The loop returns the stable artifacts; the callback retains that pass's
classification in the caller for report emission.

Cycle history retains a Git object digest for each complete version/artifact
state rather than retaining serialized lockfiles and manifests for every pass.
Input fingerprint fields use fixed-width little-endian lengths so changing the
process pointer width does not change their encoding.

The final prospective checkout is retained for external compatibility tools.
Its evidence manifest path is separate from the original input identity used by
application. Compatibility tooling uses both that manifest and its working
directory; a subsequent read-only verification compares the retained checkout
with the captured state. Generated build products do not become released inputs.

The source fingerprint also includes untracked and ignored files beneath `src/`,
because they can affect a workspace build even though they are excluded from
Git-based release classification. Changing them requires fresh compatibility
evidence, not an additional release reason.

The resolved artifact binds the original input snapshot to captured output
files, not to temporary workspace paths. Application recognizes either the
original snapshot or the fully applied snapshot. It rejects other states before
writes and installs captured bytes without invoking Cargo resolution. Lockfile
maintenance is independent of binary relevance: a library-only workspace still
receives a consistent resolved lockfile after version rewrites.

Capture and application canonicalize the selected manifest before Cargo discovers
workspace paths. Prospective directories are normalized after creation as well.
This keeps Windows short-name spellings from being mixed with canonical roots
when paths are rebased, without assuming filesystem case sensitivity.

## Diagnostics

All repository-controlled names pass through one quoting helper modeled after
Git's `core.quotePath` output. Quotes, backslashes, and control characters are
escaped so a path cannot forge another terminal line or GitHub workflow command.
GitHub command properties receive their additional delimiter escaping.

Classification emits lazy notes through a diagnostic sink. Its stderr adapter keeps
the ordinary verbose toggle; recording sinks let unit tests observe emission
conditions and computed values without process-global output capture. Status
explanations consume the completed classification and do not decide its verdict.

Renderer tests compare GitHub annotations with the generated plain diagnostics,
including the manifest destination for unpublished version-group members. They
assert structured fields and scenario values rather than freezing advisory prose.

Subprocess stderr remains intact because Git and Cargo already quote their own
paths, and escaping the entire diagnostic would destroy its multiline structure.

Operational conditions are private `ohno::error` leaves carried through
`ohno::AppError`, preserving command, parse, and filesystem causes.

## Performance boundaries

Process startup and Git/Cargo work dominate end-to-end latency and are not useful
benchmark targets: they measure the host, repository, and caches more than this
tool. Benchmarks instead isolate deterministic in-process work whose cost can
scale with workspace size:

* bounded patch rendering, across low and high edit distances; and
* lockfile parsing and repeated dependency-closure walking, across small and
  large graphs.

Criterion tracks wall-clock behavior without subprocess or filesystem noise.
Callgrind is not used because both measured paths allocate variable-sized output
or parse state, and its fixed allocator model would omit a material part of their
cost. The benchmark-only surface is available in `crp_impl` unit-test builds and through
its `private-test-util` feature, but does not participate in normal builds. Small unit tests
exercise the adapters' byte and line statistics, root selection and repeated-walk
totals without running a benchmark harness. Both adapters and their underlying
algorithms participate in library-only mutation testing; benchmark smoke runs
separately exercise the measured workloads without collecting measurements.
