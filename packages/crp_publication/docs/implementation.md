# Publication coordination

This private component implements the [application design](../../cargo-release-plan/docs/design.md)
within the [application architecture](../../cargo-release-plan/docs/implementation.md).
It owns configuration, publication intent, remote delivery and operator recovery.

Registry/OIDC and GitHub adapters supply observations and perform authorized writes.
Publication owns missing-work selection, tag/release policy, binary asset completeness
and retry decisions. Workspace supplies source facts; versioning supplies candidate
release checks; native executes validated build and supervised-process requests.

Publication manifests, platform batches and phase/item receipts have one owner here.
Native build inputs are non-wire execution values, not copies of publication envelopes.
The coordinator carries native execution results into the existing receipt projection
and retains both primary and cleanup failures. Receipt selection remains qualified by
publication, run, attempt and the batch referenced by reconciliation.

GitHub upload credentials are separate from native build state. Scoped source-fetch
authentication is supplied only to repository acquisition. Upload and its completeness
recheck share native's existing item deadline and canonical controller directory;
they never renew the budget. Initial asset discovery has its separate query budget.
Cancellation and process-group ownership remain with native.

Candidate policy consumes workspace facts and the typed versioning check result,
not application command dispatch. Production entry points select concrete adapters;
unit tests inject narrow observations/ports. Real HTTP/Git/Cargo and delivery-adapter
behavior belongs to boundary tests, never production registry or GitHub writes.
