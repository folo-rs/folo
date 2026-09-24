# backfill

`backfill` reconstructs history by checking out each commit in a range and running
[`collect`](collect.md) for it — bootstrapping an existing repository's timeline, filling the
gaps a mixed pool of benchmark machines leaves in any one machine's series, and also a
convenient path for ad-hoc evaluation over a span of commits.

```console
cargo bench-history backfill --local=./bench-history <from-commit> <to-commit>
```

The range endpoints are inclusive positional subjects: `<from-commit>` is the oldest commit of
the span and `<to-commit>` the newest. The tool first verifies both endpoints resolve and that
the start is a first-parent ancestor of the end, then derives the range purely from the end's
history — so backfilling does not depend on the current checkout or branch.

Commits are **processed newest-first**, prioritizing the recent history that an analysis of
the current tip reads. Without `--ignore-errors`, the run stops at the *newest* failing commit.

## Isolation and resumability

All work happens inside a dedicated **git worktree** under the temp directory rather than in
the primary checkout, so a dirty primary tree neither blocks backfill nor affects what is
measured, and an interruption leaves you exactly where you were. Between commits the worktree
is reset clean while preserving the ignored build directory for incremental speed.

By default, commits that already have a stored result are listed once up front and skipped
before their benches run, making backfill resumable and cheap to re-issue;
`--overwrite` regenerates them. A build or bench failure stops by default;
`--ignore-errors` instead continues and includes every failed commit in the end-of-run
summary. During replay, a commit without the selected project directory also counts as a
per-commit failure. Infrastructure failures always abort.

That skip check looks only at the **storage partition this run writes to** — the target triple
and auto-detected machine key this run stores under — so a commit measured on other hardware,
or for another target, never counts as already done here. Across engines it
takes the union: a commit that has a clean result for only some engines still counts as recorded
and is skipped, because nothing requires a run to produce every engine (off Linux, Callgrind
produces nothing at all). Use `--overwrite` to re-measure such a commit — for example after
adding a new bench, or after a run was killed partway through storing one commit's per-engine
results.

## Bounded passes

Use `--max-commits N` to attempt at most a positive number of commits in one invocation:

```console
cargo bench-history backfill --local=./bench-history <from-commit> <to-commit> --max-commits 1
```

The limit applies after the partition skip check, not to the range endpoints. Commits
skipped before benchmarking consume no budget. Each replay attempt counts, including a
failed build or benchmark, an empty harvest, or a duplicate discovered only while writing.
Every attempted commit finishes its repetitions and engine storage before another commit
can start. Reaching the limit returns normally after worktree cleanup and storage flushing;
it does not suppress errors or cancel a running benchmark.

The initial announcement states the pending work and attempt bound. The final summary
counts stored, existing, empty, failed and deferred commits, with the reason replay stopped:
limit reached, range exhausted or benchmark failure. Deferred commits are eligible work
not attempted, not already-recorded results.

Without `--max-commits`, replay is unlimited. Repeating a bounded pass over the same range
fills the next missing commits in the same partition; an all-recorded pass succeeds without
benchmarking. With `--overwrite`, each invocation instead starts at the newest commit
again, so the limit is not a resumable cursor. This bounds work, not elapsed time: allow
enough time for every selected attempt to complete.

## Toolchain and measurement configuration

Each commit is built with **the toolchain its own checkout selects**: the worktree is a
historical checkout, so the toolchain selection your shell exported into `cargo bench-history`
is dropped for the per-commit run and the toolchain is resolved from the checkout itself — the
`rust-toolchain.toml` at that commit when it has one, your local default otherwise. The stored
`rustc` version names the compiler that actually built the benchmarks either way.

The rest of the measurement configuration is *not* reconstructed from the commit and cannot be.
`RUSTFLAGS` and the benchmark scope flags are your intent as the caller — passing them through
to `cargo bench` is [`collect`](collect.md)'s contract, and a general-purpose tool has no way to
read a specific project's build configuration out of a historical worktree. So a commit older
than the newest change to either is measured slightly differently from a point that was
collected at the time that commit was pushed, and the difference can look like a step in the
series that no code change explains. Keep backfilled ranges recent to bound this, and treat an
unexplained step at the boundary of a backfilled span with suspicion.

## Noise reduction

`--best-of N` carries through to each commit's `collect`, applying the same min-of-N noise
reduction uniformly across the range. Use the same `N` the range's neighbors were collected
with: the reduction keeps a minimum, whose expected value falls as `N` rises, so a span
backfilled with a different count sits at a different level for every metric noisy enough to
be affected, and meets its neighbors as a step.
Each backfilled run records the count it was reduced from, like any other run.
