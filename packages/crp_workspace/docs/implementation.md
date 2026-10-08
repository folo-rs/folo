# Workspace observations

This private component supports the [application design](../../cargo-release-plan/docs/design.md)
and is composed as described by its [implementation guide](../../cargo-release-plan/docs/implementation.md).
It owns Git/Cargo observations and the representations needed to interpret them.

Git subprocesses, manifest/member/dependency discovery, inherited-key acquisition,
tracked package contents, installation declaration graphs, lockfile parsing and
closure calculation, and source/path identity share this boundary.
Git and manifest interpretation may depend on each other inside
the package. Classification, version-group decisions and publication eligibility
belong to callers. Workspace observations retain validated exact dependency edges;
versioning derives their groups.

Public exposure is a separate observation from dependency delivery. Canonical
allow-list crate names resolve to defining packages reachable through normal
workspace dependencies. Following those owners' declarations produces a sorted
transitive origin set; sharing an origin does not expose the suppliers themselves.
Whole-owner declarations conservatively retain nested exposure through private
implementation packages without compiling during classification.

Git observations can resolve commits and test ancestor relationships without
rewriting history. Whether a descendant snapshot represents an anticipated squash
predecessor is versioning policy, not a workspace acquisition rule.
Ancestry exit-status interpretation stays in process: Git's positive and negative
answers are distinct from execution failures, including signal termination.
Historical manifest acquisition batches recorded blob identities through `git cat-file`.
Requests never encode paths as lines, and responses retain their exact byte lengths.
Callers decode only the manifests they interpret. Captured-input subprocesses feed
stdin concurrently with draining stdout and stderr, so large batches cannot deadlock
on opposing pipe buffers; child failures retain their diagnostics.

Path handling probes actual filesystem alias behavior rather than assuming case
sensitivity from the operating system. Dependency membership uses the lexical member
index first, then filesystem-resolved member identity, so caller path spelling does
not remove dependency edges. Shared artifact-file operations own path
resolution and atomic promotion, while callers own serialization and overwrite policy.
Symbolic links are unsupported: Git modes and direct file metadata identify released
links for rejection. General filesystem identity resolution still protects case and
short-name aliases and artifact write locations; it is not a symlink support protocol.
Repository-controlled display strings use the diagnostic component's presentation helpers.
The command boundary also supplies the fixed orchestration credential names shared by
native, compatibility and registry compilation; each adapter owns environment mutation.

## Operation-owned observations

Git's full recorded trees and raw parent-header facts are shared by resolved object
identity and effective object interpretation within an operation. Full-tree path and
mode/object indexes are constructed once and retained by the snapshot owner. Refs,
traversal, parent availability and shallow verdicts are acquired freshly.

Manifest documents retain exact dependency strings and TOML value/table shapes.
The owner reuses parsed syntax only after acquiring the complete current text; it
retains neither path-dependent interpretation nor a mutable metadata snapshot.
Historical workspace interpretation requests relevant root/member syntax lazily,
deriving inheritance, compiled matchers, membership and deferred installation errors
in the bound context. Unrelated manifests are neither decoded nor parsed.
Parsed lockfile graphs serve several binary closures within a classification; committed
graphs can survive prospective passes under the owning snapshot's context.

An acquisition moves its tracked listing into the returned work tree and retains parsed
root/member documents for adjacent consumers. Read-only source traversal borrows acquired
documents and owns documents acquired on a miss. Callers keep assessed source, configuration
and history stable during read-only work; an independent command entry, deliberate mutation
or workspace relocation requires fresh acquisition. Unresolved metadata uses
`--no-deps --locked`, without turning
classification into dependency resolution. Content-keyed syntax can outlive repository
rebinding because it has no repository interpretation.

Source-location discovery accepts the same acquisition's documents and reads newly
reached manifests. Native-read and supplied-document entry points share recursive
discovery and root-alias admission. Versioning adds relocatability constraints and
fingerprints the selected files, including absent reserved inputs and transitive path
dependencies outside Cargo's member list. Explicit build-script and target paths come
from these same documents, including dependency documents. Dependency traversal deduplicates
resolved identities independently of reserved files.
Nonmember dependencies reserve their conventional build script unless explicitly disabled.
Explicit target and build-script paths share directory selection. The containing directory
is recursive only when its resolved identity is strictly below the owning package directory;
package roots, ancestors and outside directories are not recursive source roots. Declared
files remain selected in every case. Native directory resolution is injected into the
in-process selection tests. This implements the application's captured-source contract:
support files outside source directories must be tracked, without interpreting Rust modules.

Source discovery supplies captured evidence, not destination safety checks. Output placement
belongs to callers; writers do not inventory source or Git configuration to assess it.
Shared artifact operations provide atomic publication and filesystem path resolution where
required by captured source relocation and artifact ownership.

Replacement refs, replacement environment and graft contents are observed at each
classification boundary and invalidate invocation memory when they change. An
unrepresentable replacement namespace is rejected rather than treated as the default
namespace. There is no persistent observation store, cache-location discovery or
serialized interpretation. Ordinary Cargo/compiler caches remain owned by their tools.

## Patch content acquisition

Patch readers receive ordered, exact object identities, not historical paths or raw worktree
bytes. A shared length-framed size query plans byte-budgeted lookahead; repeated identities
within a batch share one payload. Advancing releases that batch, while the renderer can retain
the endpoints of its current comparison. A single oversized object is accepted alone rather
than turning the lookahead budget into a content limit. A single distinct object needs no size
query. A secondary object-count bound limits framing and retained-map overhead even for empty
blobs. Request identity, blob type, exact length and framing remain checked, and subprocess
stdin is written concurrently with output draining.
Shared vector ownership preserves each acquired payload allocation rather than copying it into
a reference-counted slice. Per-object headers and spare capacity stay with that allocation.

Every rendering pass acquires its required objects from Git. The size query rejects unavailable
objects before a multi-object read; the content protocol checks each response against its
requested identity, type and length. Singleton batches use a direct blob read. No missing object
becomes an empty file, and no payload is retained beyond its rendering consumers.

## Fresh classification listings

Each admitted interval shares ownership of its metadata acquisition's complete tracked listing.
Index modes, their worktree overlay and untracked candidates are acquired over the union of
relevant literal pathspecs, splitting arguments at the native command-line budget. Untracked
queries do not expand to unrelated repository paths. Packages select overlapping scopes, so
shared resources and outer/nested consumers retain independent packaging interpretation.
Synchronous scoped queries borrow arguments from the fixed prefix and retained pathspec strings;
batch assembly does not duplicate their owned contents.
Source capture can supply already acquired index modes; live acquisition still obtains the
worktree overlay only after effective-filter admission. Without capture, acquisition reads the
index itself. Neither path executes mode queries merely to capture source evidence.

Selection follows Git's literal component boundaries and ASCII byte case comparison, not the
filesystem's Unicode case model. Noncanonical scopes, non-ASCII insensitive scopes, overridden
pathspec environments and requests outside acquired scopes use the narrow native query.
Presence, nested-manifest removal, symlink admission and packaging rules remain in their
existing owners. Effective modes retain the index baseline followed by worktree changes.
The observation value can serve adjacent read-only consumers of the admitted classification,
including packaging selection. It is dropped before edits, resolution or relocation and is
never reused by an independent command or stored with committed snapshots.

An index entry is racily clean when its cached filesystem metadata can appear unchanged despite
a recent content change, requiring Git to compare content. Raw mode diffs can therefore execute
clean filters for such entries. Effective filter attributes are therefore acquired for the
relevant tracked paths before sharing mode queries.
If Git reports a filter attribute, the entire pass retains per-package mode-query ordering; no
assumption about driver statelessness or configuration absence permits sharing. Attribute
query failures propagate. This gate never executes a driver, does not widen to unrelated
paths, and does not change the independently shared tracked and untracked listings.
The all-attributes query distinguishes absent attributes from a driver literally named
`unspecified`; an `unset` response conservatively retains narrow acquisition because it
can also name a driver.

## Observation boundaries and tests

Pure parsing and graph tests remain in process. Real Git/Cargo/filesystem tests
belong to boundary integration targets, with hermetic Git identity/configuration.
Shared integration fixtures are opt-in private test support, not acquisition hidden
inside unit tests. `private-test-util` excludes repository creation, native filesystem
fixtures and I/O scheduling support. Inert Git constructors and the lockfile-closure
driver are ordinary public items in this private component: they only assemble captured
observations or call the existing in-process algorithm.

Observation adapters acquire process outputs, directory identities and file-type facts.
Their interpretation remains independent of native execution: status and optional-result
handling, first-parent endpoint selection, ancestor resolution, tracked-source eligibility
and configuration precedence consume captured values or narrow read callbacks. Metadata
projection shares those operations rather than duplicating publication and dependency
decisions in a test model. Only native forwarding is excluded from unit mutation testing;
boundary integrations exercise the actual commands and filesystem effects.
The shared repository fixture's Git and filesystem methods have individual
native-support mutation exclusions. I/O scheduling stays unit-tested, including
callback execution and rejection after a predecessor poisons its slot.
Inert object-context fixtures are exercised by the same identity tests as captured
contexts. Native filename fixtures probe actual creation and distinguish a specifically
unsupported encoding from unrelated I/O errors. Mode-precedence coverage does not depend
on support for non-UTF-8 filenames.
