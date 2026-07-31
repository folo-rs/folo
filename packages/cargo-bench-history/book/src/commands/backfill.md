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

Commits are **processed newest-first**. A long backfill is routinely cut short — you stop it,
or a CI job hits its time limit — and the recent end of the history is the part an analysis of
the current tip actually reads, so an interrupted run has already spent its time where it
counts. The one visible consequence: without `--ignore-errors`, the run stops at the *newest*
failing commit.

## Isolation and resumability

All work happens inside a dedicated **git worktree** under the temp directory rather than in
the primary checkout, so a dirty primary tree neither blocks backfill nor affects what is
measured, and an interruption leaves you exactly where you were. Between commits the worktree
is reset clean while preserving the ignored build directory for incremental speed.

By default, commits that already have a stored result are listed once up front and skipped
before their benches run, making backfill resumable and cheap to re-issue;
`--overwrite` regenerates them. A build or bench failure stops by default;
`--ignore-errors` instead continues and includes every failed commit in the end-of-run
summary. Infrastructure failures always abort.

That skip check looks only at the **storage partition this run writes to** — the target triple
and machine key this run stores under, `--machine-key` override included — so a commit measured
on other hardware, or for another target, never counts as already done here. Across engines it
takes the union: a commit that has a clean result for only some engines still counts as recorded
and is skipped, because nothing requires a run to produce every engine (off Linux, Callgrind
produces nothing at all). Use `--overwrite` to re-measure such a commit — for example after
adding a new bench, or after a run was killed partway through storing one commit's per-engine
results.

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
reduction uniformly across the range. Use the same `N` the range's neighbours were collected
with: the reduction keeps a minimum, whose expected value falls as `N` rises, so a span
backfilled with a different count sits at a different level and meets its neighbours as a step.
Each backfilled run records the count it was reduced from, like any other run.
