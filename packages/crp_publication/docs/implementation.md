# Publication coordination

This private component implements the [application design](../../cargo-release-plan/docs/design.md)
within the [application architecture](../../cargo-release-plan/docs/implementation.md).
It owns configuration, publication intent, remote delivery and operator recovery.

Registry/OIDC and GitHub adapters supply observations and perform authorized writes.
Publication owns missing-work selection, tag/release policy, binary asset completeness
and retry decisions. Workspace supplies source facts; versioning supplies candidate
release checks; native executes validated build and supervised-process requests.

Publication manifests, platform batches and phase/item outcomes have one owner here.
Native build inputs are non-wire execution values, not copies of publication envelopes.
The coordinator carries native execution results into persisted outcomes and retains both
primary and cleanup failures. The reporter's internal receipt projection remains qualified
by publication, run, attempt and the batch referenced by reconciliation.
The reporter decodes each phase's actual payload before projecting attempt-selection
facts. It checks inventories, execution mode and completion consistency, and retains
package and item details for the human summary. Failure evidence determines the verdict;
informational partial-success summaries do not. Binary item inventories reproduce the
package-sorted frozen batch identity even when execution grouped items by source.

Historical tag observations are reused per commit within one reconciliation. Candidate
and tag worktrees exist only while acquiring those owned facts, so their cleanup precedes
remote mutation. Registry index responses are decoded incrementally while still validating
the full response, including records after an exact-version match.

GitHub upload credentials are separate from native build state. Scoped source-fetch
authentication is supplied only to repository acquisition. Upload and its completeness
recheck share native's existing item deadline and canonical controller directory;
they never renew the budget. Initial asset discovery has its separate query budget.
Cancellation and process-group ownership remain with native.

Candidate policy consumes workspace facts and the typed versioning check result,
not application command dispatch. Production entry points select concrete adapters;
unit tests inject narrow observations/ports. Real HTTP/Git/Cargo and delivery-adapter
behavior belongs to boundary tests, never production registry or GitHub writes.

The `legacy` module adapts the retained compatibility executables to these same
publication and native capabilities. It owns their compatibility command and
batch translation, not another release-policy or execution implementation.
See the application architecture for the adapter's operational-cutover boundary.
