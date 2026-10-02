# cbh_analyze implementation

`cbh_analyze` implements the query and mutation behavior specified by the
[`cargo-bench-history` design](../../cargo-bench-history/docs/DESIGN.md). Its place in the
application is defined by the
[`cargo-bench-history` implementation guide](../../cargo-bench-history/docs/implementation.md)
and the workspace rules for [implementation documentation](../../../docs/implementation.md).

This crate owns query and mutation orchestration: selecting and loading stored data, resolving
repository history, coordinating blessings and pruning, and assembling requested outputs. Shared
dataset-selection capabilities keep the query commands aligned where the application contract
requires common behavior. It delegates I/O-free series construction and detection to `cbh_detect`
and report presentation to `cbh_render`.

Analysis carries the renderer-owned outcome with the rendered report bundle so the shell can
expose it in process or write the requested outcome file. It uses the
[shared projection](../../cbh_render/docs/implementation.md), not an orchestration-specific
mapping from findings and series coverage.

Verbose branch diagnostics consume the detector's recorded preparation decisions and blessing
provenance rather than rerunning regime selection. Each withheld series has one explanation;
the census supplies aggregate coverage and deduplicated conditional guidance. History diagnostics
use the shared count-based testability predicate. Neither path adds report fields or changes
detection policy.

The public command entry points own production wiring: they resolve and construct the configured
storage, repository, diagnostics, environment, time, and task-execution capabilities before
delegating. Their inner `*_with` orchestrators receive generic ports and explicit runtime values,
which keeps policy deterministic and same-crate tests in memory. The component crates own the
adapter implementations; `cbh_analyze` selects and coordinates them.

The query entry points acquire the quota-respecting processor count from `many_cpus` alongside
the production spawner, including processors across Windows processor groups. The process-wide
default set respects hard resource limits rather than inheriting the calling thread's soft
affinity: tasks may execute on other runtime threads. This query does not change thread affinity
or runtime sizing. The processor set is nonempty. That nonzero capacity is passed through
dataset loading and detection, each capping its worker count at its input length. Inner
orchestrators and their in-memory tests supply execution capacity explicitly rather than
discovering hardware during partitioning. Seeded-history integration tests exercise the real
command wiring and Tokio execution without running a benchmark engine.

Every command selects the ordinary storage facade. Read-only queries synchronize its optional
cloud cache before the shared selection pipeline and report cache activity afterward.
Git-topology selection separates the target branch's measurements from the base-ref history
within that store; unrelated branch commits do not enter the comparison.

Operations cross the crate boundary through a transparent aggregate. Concrete conditions remain
private to the responsibility that owns their context, while component failures remain attached
as sources. The shell can therefore convert the aggregate into `ohno::AppError` without exposing
an internal taxonomy or losing causal diagnostics. The boundary follows the workspace
[error-handling guide](../../../docs/error-handling.md).

## Preparation benchmarks

`cbh_analyze_preparation` measures preparation without running detection or rendering.
The candidate workload calls the production filter on an already-listed project's keys,
separating selection from storage access and diagnostic timing. Each repeated batch contains
selected clean and dirty runs, a machine-relaxed clean sibling, and exclusions for machine,
engine, target triple, malformed key and non-JSON suffix. Only the number of batches varies.
Both Criterion and Callgrind use this workload.

The folding workload supplies uncompressed JSON through `MemoryStorage` and explicit topology
to the production chunked loader. Its synchronous spawner executes every task inline: varying
worker capacity measures partitioning and recombination, not parallel throughput. It includes
UTF-8/JSON parsing, series accumulation, run tallies and admission flags, plus the fake's key
validation, sorted-map lookup, locking and byte copy. Those fixture costs are not a proxy for
real storage performance; compression, real adapters and topology discovery are absent.
The returned builder is not finished in measurement, keeping the workload focused on streaming
folding rather than the detector's series-finalization workload.

History length and benchmark population scale independently from a shared low case. Machine
partitions, clean/dirty observations and metric count stay fixed. The uneven multi-worker
case splits a commit's observations between partial results, exercising overlapping tally and
series merges. Setup checks the selected key identities, per-set and per-commit counts, dirty
exceptions and every resulting point's topology, provenance, ordinal and value. Fixture
construction and input cloning are outside measurement, and both harnesses consume the complete
results rather than only their sizes.

Folding intentionally has no Callgrind counterpart: the production series builder uses
randomized hash tables for commit interning and series grouping, making instruction counts
vary on unchanged source. Benchmark access must not substitute a different hashing policy or
alter storage/detection semantics to manufacture deterministic counts.

The `private-test-util` feature is a compilation boundary, not an API-hiding mechanism.
It excludes the shared synthetic-history generation and full output-verification machinery
from ordinary library builds, including the generic loader's in-memory specialization.
These fixtures require the detector/storage private features to compile; ungating them
would require enabling those features on production dependencies as well, compiling the
detector's synthetic-example catalogue and the in-memory storage implementation.
The application shell does not forward the feature. Production selection and folding remain
unconditionally compiled; lightweight access to existing implementation items alone would
not justify a gate under the [workspace guidance](../../../docs/impl-crate-split.md#internal-only-testbench-helpers-private-test-util).
