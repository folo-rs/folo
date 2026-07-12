# bless / unbless

A **blessing** manually accepts an intentional performance change on the base branch so
history analysis stops re-flagging it. Sometimes a regression is a deliberate tradeoff, and
without a way to record that, every subsequent [`analyze`](analyze.md) would keep reporting
the same accepted step forever. Blessing re-baselines the series from the blessed commit
forward.

```console
# Accept a change on one or more benchmarks (matched by id prefix) at a base-branch commit.
cargo bench-history bless --local=./bench-history <benchmark-prefix>...

# Remove the blessings recorded at the context commit.
cargo bench-history unbless --local=./bench-history <benchmark-prefix>...
```

## Rules

- `bless` takes one or more benchmark-id prefixes matched against the qualified identity, so
  it is deliberately per-benchmark — accepting the benchmark that caused trouble must not
  silently accept every other benchmark that may be trending badly. An all-switch (mutually
  exclusive with prefixes) accepts every benchmark recorded at the commit.
- Both commands operate on a context ref (default `HEAD`), so any base-branch commit can be
  (un)blessed, not just the checked-out one.
- A blessing is recorded only when the context commit is **on the base branch** and a clean
  run already exists there; both are hard errors otherwise. A dirty working tree is allowed
  (the blessing targets the committed run) but warns.

## Lifecycle

A blessing is an append-only sidecar alongside the commit's clean run, so narrowing one means
unbless-then-re-bless the subset to keep, and overwriting a commit's clean run drops its stale
sidecars. `unbless` deletes only the blessings recorded at the context commit; blessings at
later commits stay in effect. Blessings are honoured **only in history mode** — branch mode
judges the latest state against the base, which is treated as fully blessed by construction.

Use [`list blessings`](list.md) to audit which blessings are in effect.
