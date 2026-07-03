# Agent notes for cargo-bench-history-core

This crate holds the pure data-model and **analysis** internals of
`cargo-bench-history` (no I/O). The `analyze` command loads up to millions of
stored runs and reduces them to per-series findings, so the code below the IO
ports is on a genuinely hot path. Treat performance as a first-class concern
here, alongside the workspace-wide rules in `docs/` (especially
`docs/performance.md` and `docs/testing.md`).

## Performance principles for the analysis pipeline

* **Keep the fold compact; each worker folds its own chunk.** The series fold
  (`analyze::series::SeriesBuilder`) extracts each run's compact points and drops the
  parsed run immediately, so the points kept — not the run's full JSON shape — bound
  what the fold retains. The shell loader fans the per-object decompress + JSON parse +
  fold across cores (see the distribution bullet below): each worker folds its chunk
  into its *own* `SeriesBuilder`, dropping each parsed run as it goes, and the main
  thread merges the per-worker builders (`SeriesBuilder::merge`). No worker buffers its
  chunk's parsed runs. Each object is parsed into the lean
  [`RunPoints`](src/analyze/run_points.rs) projection — only the fields
  `SeriesBuilder::push` reads (per result, the id and the
  metric value/interval), dropping the run context and per-metric standard deviation —
  and `push` consumes that projection rather than a full `Run`. The full commit ID a
  point is labelled with comes from the storage key, not the run payload, so the
  projection carries no git fields. Keep that projection in
  lockstep with what the fold reads. **Note on memory:** folding in the worker did *not*
  lower peak — at merge time every worker's finished builder is resident alongside the
  growing combined builder (~2× the compact point output transiently), which measured
  ~5% above a buffered-parallel variant; it is kept for the ~10% wall-time and CPU win.
  The lean `RunPoints` element likewise trims peak only ~1%. The repeated benchmark ids
  and merge coexistence dominate peak, so the open memory levers are bounded
  merge-as-complete waves and id interning, not a leaner element — see the load
  section of `docs/analyze.md`.
* **Operate in place; avoid hidden allocations.** Prefer
  [`sort_unstable_by`](slice::sort_unstable_by) over `sort_by` for the statistics
  primitives: the stable sort allocates a scratch buffer of up to `len`
  elements, while the unstable sort runs in place. Median/rank/tie computations
  resolve ties explicitly (or compare bit-identical `f64`s), so stability is
  never required — `analyze::stats` uses the unstable sort throughout.
* **Reuse memory and preallocate to known sizes.** Size buffers up front when the
  count is known (e.g. `theil_sen_line` reserves the exact `n·(n−1)/2` pairwise
  slopes via `pair_count`, so the fill loop never reallocates), and drain-and-reuse
  a working buffer across iterations rather than reallocating it. The gzip codec
  (`codec`) holds per-thread inflate/deflate state and resets it between calls, so
  each thread allocates the working state once and repeated calls — including from
  worker threads — stay allocation-free; keep this reuse intact.
* **Distribute independent compute through an injected `Spawner`, not ad-hoc
  threads.** Two passes fan out across cores this way. The shell's object loader
  (`cargo-bench-history`'s `analyze::fold_runs_chunked`) splits the storage-key-sorted
  survivors into balanced contiguous chunks and parses + folds each chunk into a
  per-worker `SeriesBuilder` on a spawned task, then merges the partials. The detection
  step (`find_changes_spawned`) likewise splits the series into one
  balanced contiguous chunk per worker — sizes from
  [`analyze::parallel`](src/analyze/parallel.rs)
  (`worker_count` + `balanced_chunk_sizes`) — and dispatches each chunk to a blocking
  task on an injected `anyspawn::Spawner`, then awaits and concatenates in series order
  so the output equals a sequential pass. Production injects a Tokio spawner (sharing
  the runtime's blocking pool); tests and Miri inject
  [`testing::synchronous_spawner`](src/testing.rs), which runs each task inline on the
  calling thread, so the analysis needs no runtime and stays deterministic under
  `block_on`/Miri. A single available CPU (the Miri default) yields a single worker —
  one chunk, one task over every series — not a separate serial branch. New per-series
  logic must stay
  side-effect-free so it can run on any worker; do not introduce shared mutable state
  into a distributed pass. The per-worker builder merge (`SeriesBuilder::merge`) and
  per-series point sort (`SeriesBuilder::finish`) stay **serial** — their work is
  dominated by task overhead — and run after the parallel loader's per-worker folds are
  awaited; the merge is associative and `finish` sorts globally, so the result equals a
  single-threaded fold in storage-key order.

When you add a hot-path optimization, justify any deviation from a standard
ecosystem pattern (see `docs/performance.md`) and cover the numeric boundaries
with named, value-asserting tests rather than threshold guards (see
`docs/testing.md`). All of `analyze::stats` must stay deterministic and
Miri-safe.
