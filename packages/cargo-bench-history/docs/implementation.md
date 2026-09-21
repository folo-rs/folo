# cargo-bench-history implementation

User-visible behavior belongs in the [application design](DESIGN.md). This guide applies the
workspace rules for [implementation documentation](../../../docs/implementation.md) to the
application's package boundaries.

## Ownership map

The application crate is the composition shell. It owns typed-command dispatch, passes command
options and test overrides into the owning command entry points, handles final output destinations,
and owns the process entry point. It selects and wires concrete adapters for command families it
drives directly; the analysis command family delegates that responsibility to `cbh_analyze`. The
private-use crates own the capabilities composed into the application:

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
  including data loading and selection around detection and rendering. Its public command entry
  points select the production capabilities before delegating to generic inner orchestrators.
* `cargo-bench-history-figures` owns the generated-evidence infrastructure for the book: the asset
  registry, fixtures derived from production projections, presentation styles, and preview
  rendering. It is book infrastructure, not part of the application binary; dependencies run from
  the generator to the production projections, never from the shipped application to the generator.
These boundaries are directional: component crates do not depend on the shell, and behavioral
policy remains with the application even when a component implements it. More detailed analysis
data flow is documented in the [analysis implementation guide](analyze.md).

The renderer owns the shared analysis-outcome projection. Query orchestration carries the
outcome with its rendered reports, and the shell exposes the typed value or writes the
requested outcome file without introducing another decision rule. Projection ownership is
described in the [renderer guide](../../cbh_render/docs/implementation.md).

Report-file orchestration preflights all destinations through its output port before the first
write. The filesystem adapter resolves existing ancestors before missing path components and
compares existing objects by file identity, including hard links. Prospective names are probed in
their actual parent filesystem using an owned scratch tree; output parents are not created during
preflight. Probe destinations are files, and ancestor identity checks reject a report file that
would need to act as another report's directory in either output order. The checks preserve
symlink traversal and filesystem-specific name equivalence without assuming case sensitivity
from the OS. An unresolvable existing link is an inspection error,
not evidence that destinations differ. Native integration tests exercise these filesystem rules;
in-memory port tests cover collision decisions and the absence of earlier writes.

## Implementation tenets

Pure transformation and decision logic remains synchronous in component crates. External work is
kept behind narrow asynchronous ports. The real entry point that owns each command family selects
its production adapters: the shell does so for families it drives directly, while the public
`cbh_analyze` entry points construct the analysis diagnostics, storage, repository, environment,
runtime clock, and Tokio task-execution capabilities. The inner `*_with` orchestrators receive
generic ports and resolved values, with deterministic substitutes used by component tests. This
keeps orchestration independent of a particular process, filesystem, storage service, clock, or
task executor.

Storage selection wires one backend, with an optional read-through cache for Azure. PR and
trunk measurements use the same persistence path; query-time topology selection keeps
unrelated branch commits out of trunk series. Local filesystem storage remains an alternative
backend selected at run time.

Error boundaries match the context each component owns. Semantic operations expose package
aggregates where callers need a component-level boundary. Lower-level components instead return
the foreign error that describes their mechanism: process and probe boundaries use `io::Result`,
codec decoding uses `io::Error`, and model or analysis-projection JSON conversion uses
`serde_json::Error`. The caller that knows the attempted operation adds semantic context before the
failure reaches the shell.

Shell-owned conditions and contextualized component failures enter `ohno::AppError`. Concrete
conditions remain private to the layer that owns their context, and lower-level causes remain
attached rather than being flattened. The shared conventions are defined by the workspace
[error-handling guide](../../../docs/error-handling.md). The shell exposes no test-support
constructors: fixtures that need a concrete backend build it through the owning component's
public constructor and inject it as an override.

Integration-only benchmark engines and stress tools remain outside the production dependency
boundary. They drive the same public shell or persisted format without adding test-only behavior
to the shipped application.

Backfill asks Git for the selected project's repository-relative prefix instead of comparing
absolute paths with filesystem case assumptions. It accepts only a relative descendant path
(or the empty prefix for a root project), then roots both the partition pre-check and per-commit
collection at that location in the temporary worktree. The benchmark runner, environment probe,
and target output share this project directory; Git checkout and cleanup remain repository-wide.

## Azure provisioning bundle

The shell owns `setup-azure` execution and export; `cbh_cli` parses its arguments and
`cbh_command` carries the typed options, following the ordinary command boundary. It does not
construct benchmark storage, probe the measured machine, or resolve a Git checkout. Its
behavioral contract is [Azure setup](DESIGN.md#710-setup-azure).

One package-owned bundle contains the Bicep resource definitions, parameter template, deployment
script and its PowerShell module dependencies. The binary embeds these files at compile time.
They live in `src/azure_bundle/` so the ordinary package allow-list includes them; a
registry source install must not rely on repository-root `infra/` or `scripts/` files.
The export is self-contained, with bundle-relative imports and no dependency on `constants.env`.
Folo's production and test infrastructure wrappers supply their separate placement and
identity parameters to the source-built command. Neither wrapper owns a separate Bicep
template or deployment path; both exercise the same embedded bundle and preservation policy.

Bicep owns resource definitions. The single PowerShell driver owns Azure CLI discovery,
state-preserving bootstrap decisions and deployment of the selected identity with branch
and PR federation.
Rust owns parameter validation, the PowerShell prerequisite, bundle materialization, process
invocation and output/error handling, not a second implementation of those Azure decisions.
The standalone driver verifies Azure CLI, installed Bicep and an authenticated enabled
subscription (including token acquisition) before any resource mutation. `AzureDeployment.psm1`
owns this shared boundary; wrappers do not call its internal prerequisite helpers.
Both production and test deployments preserve existing storage settings. Test execution creates
and cleans its isolated containers independently of provisioning.
Current-user resolution checks that the active Azure CLI subscription
and tenant match the explicit target before calling `az ad signed-in-user show`, whose
directory lookup uses the active context. A mismatch reports how to select that subscription;
the driver does not change persisted CLI defaults or handle raw secret tokens.
The user account check and object-ID validation precede resource-group creation.
JSON parameter
values remain literal data; script flags may explicitly override them for standalone use.
The current-user switch is execution-only, not a new value type in the exported parameter file.
Callers serialize invocations sharing a storage account or managed identity because discovery
and subsequent writes address those shared resources, including the identity's
federated-credential collection. Unique Azure deployment names do not coordinate these writes.
The PowerShell boundary is deliberate: an exported bundle remains independently editable and
executable with Azure tooling, without a Rust toolchain or this application. Porting the driver
to Rust solely to remove `pwsh` would require a replacement standalone deployment path; that
additional maintenance is not justified by this command.

All prerequisite probes precede mutations. Process arguments are passed structurally rather
than interpolated into executable shell text, and the chosen subscription is explicit on Azure
operations. Export bypasses process and credential adapters entirely. Filesystem/process ports
let in-process tests prove dispatch, argv, prerequisite ordering and error propagation without
starting tools. Native integration tests cover temporary-directory ownership, export destinations
and execution of an extracted bundle; deployment policy retains its mocked-Azure coverage.

The workspace tests exercise export and standalone-driver behavior through the workspace-built
binary and mocked Azure. Wrapper tests verify that both Folo parameter sets reach the same
command, and fresh/repeated deployment behavior shares one policy suite. These runtime tests
do not establish a packaged-source build or Bicep compilation.

Offline Bicep validation checks the shared templates and bundle imports. Packaged-source
build/export checks validate distribution separately; registry-source installation checks
run after their required dependencies are published. None of these checks deploys live
resources; provisioning remains an explicit maintainer action.
