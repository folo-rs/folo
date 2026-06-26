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
  a working buffer across iterations rather than reallocating it (the shell loader
  reuses one batch `Vec` across every parse batch). The gzip codec (`codec`) holds
  per-thread inflate/deflate state and resets it between calls, so each thread
  allocates the working state once and repeated calls — including from worker
  threads — stay allocation-free; keep this reuse intact.
* **Parallelize independent passes with the shared helpers.** Several passes walk a
  slice whose elements are independent; route them through
  [`analyze::parallel`](src/analyze/parallel.rs) (`map_parallel` /
  `for_each_mut_parallel`) rather than hand-rolling scoped threads. `detect_all`
  detects each series, `SeriesBuilder::finish` sorts each series' points, and the
  shell crate's loader parses each fetched batch of runs — all on those helpers,
  which keep input order, fall back to a serial pass for a single worker (the Miri
  default, which is what keeps the logic under Miri's checks), and propagate worker
  panics. New per-series (or per-element) logic must stay side-effect-free so it can
  run on any worker; do not introduce shared mutable state into a parallel pass.
  Decompression deliberately stays serial inside `Storage::get` so the in-memory
  test fake and the gzip backends remain interchangeable — parallelize the parse and
  the analysis around it, not the decode.

When you add a hot-path optimization, justify any deviation from a standard
ecosystem pattern (see `docs/performance.md`) and cover the numeric boundaries
with named, value-asserting tests rather than threshold guards (see
`docs/testing.md`). All of `analyze::stats` must stay deterministic and
Miri-safe.
