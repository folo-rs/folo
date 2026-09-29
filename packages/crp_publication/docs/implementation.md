# Publication coordination

This private component implements the [application design](../../cargo-release-plan/docs/design.md)
within the [application architecture](../../cargo-release-plan/docs/implementation.md).
It owns configuration, publication intent, remote delivery and operator recovery.

Registry/OIDC and GitHub adapters supply observations and perform authorized writes.
Publication owns missing-work selection, tag/release policy, binary asset completeness
and retry decisions. Workspace supplies source facts; versioning supplies candidate
release checks; native executes validated build and supervised-process requests.

Binary discovery checks the selected target and inexpensive publication metadata only.
Cargo's existing explicit default-feature build owns feature resolution and buildability;
preparation does not predict those results with another resolver or early build.
Author-selected binaries can therefore fail during native execution, where the item outcome
retains the actual Cargo failure without suppressing independent items.
Preparation retains tracked, unchanged source and lockfile inputs without resolving
Cargo's full dependency graph in advance. Registry publication and native builds keep
their locked Cargo execution. Cargo owns package construction, verification and upload;
publication does not reread Cargo's archive or compare its normalized lockfile with the
source installation closure. Released-content classification retains its own source identity.

Publication manifests, platform batches and phase/item outcomes have one owner here.
Native receives in-process build requests derived from validated publication data,
not copies of serialized publication manifests or batches.
The coordinator carries native execution results into persisted outcomes and retains both
primary and cleanup failures. The reporter's internal receipt projection remains qualified
by publication, run, attempt and the batch referenced by reconciliation.
The reporter decodes each phase's actual payload before projecting attempt-selection
facts. It checks inventories, execution mode and completion consistency, and retains
package and item details for the human summary. Failure evidence determines the verdict;
informational partial-success summaries do not. Binary item inventories reproduce the
package-sorted frozen batch identity even when execution grouped items by source.
Each internal receipt keeps its input path. Collection retains independent valid outcomes
alongside per-file failures; ambiguous execution units identify their conflicting paths
without discarding unrelated results. The full report remains an artifact, while issue
delivery uses a bounded summary with an explicit omission notice and run reference.

Historical tag observations are reused per commit within one reconciliation. Candidate
and tag worktrees exist only while acquiring those owned facts, so their cleanup precedes
remote mutation. Registry index responses are decoded incrementally while still validating
the full response, including records after an exact-version match.
Post-upload observation retains each independent package result even when another lookup
fails. Failed writes or Cargo exits remain attached when confirmation also fails.
Shared source-intent verification and outcome-file persistence belong to the publication
source and artifact modules rather than to a registry-specific adapter.
Tracked inputs are collected within each guarded acquisition step and checked through
workspace's batched index API; canonical paths are reused during intent serialization.
Registry preflight consumes typed plan-inspection facts and the shared reserved-metadata
and destination-eligibility rules, rather than a second schema or a JSON round trip.

Credential issuance validates Cargo's publish operation, registry, requested package/version
and checksum shape, with a clean-source check before acquiring per-upload authority.
Cargo computes that checksum from the archive handle it uploads. The provider does not
authenticate archive contents against source or enforce an additional packaged dependency
closure. Private token leases, revocation and independent cleanup failures remain session-owned.

GitHub upload credentials are separate from native build state. Scoped source-fetch
authentication is supplied only to repository acquisition. Upload and its completeness
recheck share native's existing item deadline and canonical controller directory;
they never renew the budget. Initial asset discovery has its separate query budget.
Cancellation and process-group ownership remain with native.

Candidate policy consumes workspace facts and the typed versioning check result,
not application command dispatch. Production entry points select concrete adapters;
unit tests inject narrow observation and action interfaces. Real HTTP/Git/Cargo and delivery-adapter
behavior belongs to boundary tests, never production registry or GitHub writes.
