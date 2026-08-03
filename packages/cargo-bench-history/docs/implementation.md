# cargo-bench-history implementation

User-visible behavior belongs in the [application design](DESIGN.md). This guide applies the
workspace rules for [implementation documentation](../../../docs/implementation.md) to the
application's package boundaries.

## Ownership map

The application crate is the composition shell. It owns typed-command dispatch, application-level
orchestration, concrete adapter wiring, output destinations, and the process entry point. The
private-use crates own the capabilities composed directly or indirectly by that shell:

* [`cbh_cli`](../../cbh_cli/docs/implementation.md) owns argument parsing, help rendering, and
  early exits before command execution.
* [`cbh_command`](../../cbh_command/docs/implementation.md) owns the dependency-light command and
  option values exchanged between parsing and execution.
* [`cbh_diag`](../../cbh_diag/docs/implementation.md) owns the shared diagnostic-reporting
  abstraction and diagnostic text helpers.
* [`cbh_model`](../../cbh_model/docs/implementation.md) owns the shared I/O-free domain,
  persisted-record representation, and persisted object-key layout, construction, and parsing.
* [`cbh_config`](../../cbh_config/docs/implementation.md) owns configuration loading and the
  resolution of command inputs into concrete configuration values.
* [`cbh_git`](../../cbh_git/docs/implementation.md) owns subprocess execution and read-only
  repository-topology access.
* [`cbh_probe`](../../cbh_probe/docs/implementation.md) owns environment, toolchain, and hardware
  observation and machine fingerprinting.
* [`cbh_engines`](../../cbh_engines/docs/implementation.md) owns adapters from benchmark-engine
  environments and artifacts to the shared model.
* [`cbh_codec`](../../cbh_codec/docs/implementation.md) owns the stored-object byte encoding.
* [`cbh_storage`](../../cbh_storage/docs/implementation.md) owns the persistence port, backend
  adapters, caching, storage-facing key validation, cache-control keys, and cache invalidation.
* [`cbh_stats`](../../cbh_stats/docs/implementation.md) owns reusable statistical kernels without
  application analysis policy.
* [`cbh_detect`](../../cbh_detect/docs/implementation.md) owns I/O-free series reconstruction,
  detection, and finding production.
* [`cbh_render`](../../cbh_render/docs/implementation.md) owns report presentation and formatting.
* [`cbh_analyze`](../../cbh_analyze/docs/implementation.md) owns query and mutation orchestration,
  including data loading and selection around detection and rendering.

These boundaries are directional: component crates do not depend on the shell, and behavioral
policy remains with the application even when a component implements it. More detailed analysis
data flow is documented in the [analysis implementation guide](analyze.md).

## Implementation tenets

Pure transformation and decision logic remains synchronous in component crates. External work is
kept behind narrow asynchronous ports, with production adapters selected by the shell and
deterministic substitutes used by component tests. This keeps orchestration independent of a
particular process, filesystem, storage service, clock, or task executor.

Error boundaries match the context each component owns. Semantic operations expose package
aggregates where callers need a component-level boundary. Lower-level components instead return
the foreign error that describes their mechanism: process and probe boundaries use `io::Result`,
codec decoding uses `io::Error`, and model or analysis-projection JSON conversion uses
`serde_json::Error`. The caller that knows the attempted operation adds semantic context before the
failure reaches the shell.

Shell-owned conditions and contextualized component failures enter `ohno::AppError`. Concrete
conditions remain private to the layer that owns their context, and lower-level causes remain
attached rather than being flattened. The shared conventions are defined by the workspace
[error-handling guide](../../../docs/error-handling.md). Hidden test-support constructors exposed
by the shell use the same application boundary as command execution.

Integration-only benchmark engines and stress tools remain outside the production dependency
boundary. They drive the same public shell or persisted format without adding test-only behavior
to the shipped application.
