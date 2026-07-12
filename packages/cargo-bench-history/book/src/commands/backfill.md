# backfill

`backfill` reconstructs history by checking out each commit in a range and running
[`collect`](collect.md) for it — bootstrapping an existing repository's timeline, and also a
convenient path for ad-hoc evaluation over a span of commits.

```console
cargo bench-history backfill --local=./bench-history <from-commit> <to-commit>
```

The range endpoints are inclusive positional subjects. Commits are enumerated oldest-first
along the first-parent mainline; the tool first verifies both endpoints resolve and that the
start is a first-parent ancestor of the end, then derives the range purely from the end's
history — so backfilling does not depend on the current checkout or branch.

## Isolation and resumability

All work happens inside a dedicated **git worktree** under the temp directory rather than in
the primary checkout, so a dirty primary tree neither blocks backfill nor affects what is
measured, and an interruption leaves you exactly where you were. Between commits the worktree
is reset clean while preserving the ignored build directory for incremental speed.

By default, commits that already have a stored result are listed once up front and skipped
before their benches run, making backfill resumable and cheap to re-issue; overwrite
regenerates them. A build or bench failure stops by default (or, with a flag, is recorded
and skipped with an end-of-run summary), while infrastructure failures always abort.

`--best-of N` carries through to each commit's `collect`, applying the same min-of-N noise
reduction uniformly across the range.
