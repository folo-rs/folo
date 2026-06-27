# Agent notes for cargo-bench-history-core

This crate holds the pure data-model and **analysis** internals of
`cargo-bench-history` (no I/O). The `analyze` command loads up to millions of
stored runs and reduces them to per-series findings, so the code below the IO
ports is on a genuinely hot path. Treat performance as a first-class concern
here, alongside the workspace-wide rules in `docs/` (especially
`docs/performance.md` and `docs/testing.md`).

## Performance principles for the analysis pipeline

* **Stream, don't accumulate.** Fold each fetched run into compact series points
  and drop the parsed run immediately (`analyze::series::SeriesBuilder`); never
  hold the whole parsed data set resident at once. A new loader stage must keep
  this property — peak memory is bounded by the points kept, not the objects
  seen.
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
  threads.** The detection step (`find_changes_spawned`) is the one compute pass that
  fans out across cores: it splits the series into one balanced contiguous chunk per
  worker — sizes from [`analyze::parallel`](src/analyze/parallel.rs)
  (`worker_count` + `balanced_chunk_sizes`) — and dispatches each chunk to a blocking
  task on an injected `anyspawn::Spawner`, then awaits and concatenates in series order
  so the output equals a sequential pass. Production injects a Tokio spawner (sharing
  the runtime's blocking pool); tests and Miri inject
  [`testing::synchronous_spawner`](src/testing.rs), which runs each task inline on the
  calling thread, so the analysis needs no runtime and stays deterministic under
  `block_on`/Miri. A single available CPU (the Miri default) or a single series takes
  the serial path, dispatching no task. New per-series logic must stay
  side-effect-free so it can run on any worker; do not introduce shared mutable state
  into a distributed pass. The series fold (`SeriesBuilder::push`) and per-series point
  sort (`SeriesBuilder::finish`) stay **serial** — their work is dominated by task
  overhead, and the fold already overlaps the shell loader's concurrent fetch.

When you add a hot-path optimization, justify any deviation from a standard
ecosystem pattern (see `docs/performance.md`) and cover the numeric boundaries
with named, value-asserting tests rather than threshold guards (see
`docs/testing.md`). All of `analyze::stats` must stay deterministic and
Miri-safe.
