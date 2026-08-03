# cargo-bench-history - Implementation

The application shell parses commands, assembles production adapters, and converts private
application failures and component aggregates into `ohno::AppError`. Command handlers retain
application-owned conditions privately; integration tests exercise the `AppError` boundary and
observable output, while same-crate unit tests verify exact producer mappings.

The application is implemented by six focused private crates:

* [`cbh_analyze`](../../cbh_analyze/docs/implementation.md) owns selection, topology-based history
  reconstruction, analysis, blessing, pruning, and report production.
* [`cbh_config`](../../cbh_config/docs/implementation.md) owns configuration parsing and the
  resolution of command selections into paths and project identity.
* [`cbh_engines`](../../cbh_engines/docs/implementation.md) owns benchmark-engine environment
  setup, output discovery, and conversion into the shared model.
* [`cbh_git`](../../cbh_git/docs/implementation.md) owns process execution and read-only git
  history ports and adapters.
* [`cbh_model`](../../cbh_model/docs/implementation.md) owns the I/O-free stored model,
  comparability rules, and best-of-N reduction.
* [`cbh_storage`](../../cbh_storage/docs/implementation.md) owns object keys, storage ports,
  filesystem and Azure backends, and cloud read-through caching.

Command orchestration depends on small async ports at I/O boundaries and keeps parsing, mapping,
selection, and rendering synchronous. Production entry points wire Tokio-backed adapters; tests
use in-memory adapters and injected clocks so orchestration remains deterministic.

Backfill retains each failed commit together with its typed `AppError` until the summary is
rendered. A stable index identifies the failure that stopped processing, allowing the same error
to become the returned cause without duplicating rendered strings or losing stopping identity.

Component libraries expose transparent aggregate errors over private semantic condition types.
Only caller decisions required by production behavior are public; for example, storage exposes
narrow missing-object and already-existing-key queries rather than its complete internal failure
taxonomy. Every layer retains foreign errors as sources and adds only operation context it owns.
