# Implementation

Pure marker and message composition is synchronous. Lifecycle orchestration is generic over the
`GitHub` port; unit tests use an in-memory fake and `futures::executor::block_on`, with no runtime,
network or real-time delay. The production `RestGitHub` adapter is the only HTTP boundary.

The port exposes semantic GitHub operations—list, create, update, close, delete, compare and read
the pull-request head—rather than a raw HTTP passthrough. Idempotent operations retry transient
failures in the adapter. Creates never retry blindly: orchestration reads by marker after an
error and treats a matching artifact as the successful result of the ambiguous request.

The binary is a thin Clap and Tokio entry point. `lib.rs` and `main.rs` contain only crate-level
documentation, attributes, re-exports and entry-point wiring.

## Evidence and state transitions

JSON decoding produces a validated analysis report before any publication operation. The
publication boundary validates the report's mode, frozen commit, outcome and census consistency
and separately carries collection-platform coverage. Message selection uses that typed evidence;
it does not scrape Markdown or map the tool's individual unjudged-reason vocabulary.

All-clear validation happens before issue lookup or mutation. This makes a contradictory report
or a missing platform an error even when the rolling issue happens not to exist. The CLI
supplies the same evidence to publish and cleanup, avoiding independent definitions of clean.

PR placeholder ownership combines workflow run ID and frozen head. Finalization acts only on its
own in-progress marker. Freshness and distance queries are distinct: inability to compute a
distance produces an explicit qualification, while a known newer result is preserved.
History issue replacement also checks commit ordering: the same commit or a verified forward
comparison may replace existing findings, while an absent, backward or unknown ordering leaves
them intact. Both publication and all-clear share this guard.

An ambiguous create is reconciled against both the artifact identity and the desired body.
Finding the same marker with different content is not proof that this publication committed;
the other content is preserved and the original failure remains visible.

## HTTP adapter

The semantic port is implemented by a REST adapter over an injected request executor and
delay provider. Request construction, JSON decoding, pagination, status classification and
retry decisions run identically with the real executor and a scripted in-memory executor.
Tests record serialized requests and requested delays without using sockets or a clock.

Idempotent reads and updates retry only transient failures within a bounded attempt and delay
budget. An acceptable `Retry-After` delay is honored; unsupported or excessive delays are
reported rather than retried prematurely. Pagination must make progress, and a later page's
failure remains an error rather than a successful partial list. Comparison distances are
numeric only for a verified linear forward relationship or identical commits.

The reqwest boundary disables automatic redirects and retries so they cannot bypass the
adapter's operation-specific policy. Creates have no blind retry; lifecycle reconciliation
uses the same REST decoding as ordinary lookup. Credentials are redacted from diagnostic
representations. Only the actual network and timer primitives are outside in-process tests;
the request and response policy is not excluded with them.
