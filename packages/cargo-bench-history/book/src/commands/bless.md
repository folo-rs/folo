# bless / unbless

A **blessing** manually accepts an intentional performance change on the base branch so
history analysis stops re-flagging it. Sometimes a regression is a deliberate tradeoff, and
without a way to record that, every subsequent [`analyze`](analyze.md) would keep reporting
the same accepted step forever. Blessing re-baselines the series from the blessed commit
forward.

```console
# Accept a change on one or more benchmarks (matched by id prefix) at a commit.
cargo bench-history bless --local=./bench-history <benchmark-prefix>...

# Remove the blessings recorded at the context commit.
cargo bench-history unbless --local=./bench-history <benchmark-prefix>...
```

## Rules

- `bless` takes one or more benchmark-id prefixes matched against the qualified identity, so
  it is deliberately per-benchmark — accepting the benchmark that caused trouble must not
  silently accept every other benchmark that may be trending badly. `--all` (mutually
  exclusive with prefixes) accepts every benchmark recorded at the commit.
- Both commands operate on a context ref (default `HEAD`), so any commit that resolves can be
  (un)blessed, not just the checked-out one.
- Blessing prefers — but does not require — the base branch and an existing clean run at the
  commit. Blessing **off the base branch warns**: it takes effect only once the commit joins
  the base branch's first-parent history (for example after a fast-forward), so a fast-forward
  merge workflow can bless a commit already on a feature branch. Blessing a commit with **no
  recorded run also warns** (double-check the commit id) and synthesizes the target discriminant
  sets from the resolved discriminant filters — all four engines when `--engine` is omitted,
  under the resolved target triple and machine key — so a change can be accepted *before* its
  data is captured.
  A no-data blessing needs a concrete target triple and machine key, so one whose triple or
  machine-key filter is unconstrained (`all`) is an error. An unresolvable context ref, an
  undeterminable base branch, or no prefixes without `--all` remain hard errors.
- A dirty working tree is allowed (the blessing targets the committed run) but warns.

## Lifecycle

A blessing is an append-only sidecar in each targeted set's commit directory (which need not
yet hold a run), so narrowing one means unbless-then-re-bless the subset to keep. Capturing or
overwriting a run never removes a blessing. `unbless` deletes only the blessings recorded at the
context commit; blessings at later commits stay in effect.

History mode starts detection at the blessed commit while retaining earlier points for chart
context. Branch mode applies the blessing to the base ref's own first-parent evidence: the blessed
commit remains eligible and every earlier base observation is excluded from regime selection,
observed ranges, and historical report comparison. A recent blessing can therefore leave a branch
series unjudged until enough new base measurements accumulate.

Use [`list blessings`](list.md) to audit which blessings are in effect.
