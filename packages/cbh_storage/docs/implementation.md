# cbh_storage implementation

`cbh_storage` implements the persistence behavior specified by the
[`cargo-bench-history` design](../../cargo-bench-history/docs/DESIGN.md). Its place in the
application is defined by the
[`cargo-bench-history` implementation guide](../../cargo-bench-history/docs/implementation.md)
and the workspace rules for [implementation documentation](../../../docs/implementation.md).

The crate owns the persistence port, local and Azure adapters, read-through caching, storage-facing
validation of keys presented to backends, cache-control key construction, and cache invalidation.
Persisted object-key layout, construction, and parsing belong to `cbh_model`. Backend adapters
preserve one storage contract; callers and caching logic depend on the port rather than
backend-specific APIs. Configuration selection is supplied by `cbh_config`, stored values use
`cbh_model`, and byte encoding belongs to `cbh_codec`.

All backends return one operation-level aggregate. Private conditions add backend and operation
context while retaining filesystem, codec, configuration, or service causes. The aggregate exposes
only the narrow decisions required across the crate boundary: object absence and write-once
collision. Other distinctions remain private under the workspace
[error-handling guide](../../../docs/error-handling.md).

Local writes compress data and publish it atomically. Azure operations retain SDK diagnostics,
while conditional creates persist an opaque request identity so a collision caused by an automatic
retry after a committed upload is recognized as success. Read-through caching uses per-project
invalidation markers after remote overwrites and deletions.

Collection, queries and administrative commands select the same storage facade.
Cloud queries may use the read-through cache; synchronization and hit/miss accounting
belong to that facade. The analysis layer selects applicable stored commits by Git topology,
so branch measurements and the shared baseline need no separate storage view.

Azure and Azurite tests share a container-name generator behind `private-test-util`. Each
container gets a fresh random UUID, retaining its full identity in Azure's lowercase naming
format. Isolation therefore does not depend on clock precision, process-local counters, or
serialization within a test binary; concurrent processes and jobs can use the same account
without sharing test data or cleanup targets.
The `bh-it-` prefix keeps real-Azure containers discoverable by the infrastructure's
leftover-container cleanup script.

## GitHub OIDC acquisition

GitHub assertion acquisition owns a finite retry budget independently of Azure blob requests.
It retries the Azure SDK's transient HTTP status set and connection/I/O failures, including
interrupted successful response bodies. Short exponential waits give the issuer time to recover
while limiting the extra delay introduced by retries. Status rejection takes precedence over
reading an error body; token parsing and non-transient rejection do not consume retries.

The adapter reuses the shared HTTP client and SDK sleep primitive, but not the SDK retry pipeline:
that pipeline reads its own clock and can log raw transport diagnostics. A fixed delay sequence
keeps the retry budget independent of real time, and an injected sleeper makes the complete
acquisition policy testable in process. Exhaustion returns a credential error rather than
delegating further transport retries to an outer policy.

Diagnostics retain HTTP status or transport classification, not raw URLs, request headers,
response bodies or transport/parser error sources that could quote credentials.
