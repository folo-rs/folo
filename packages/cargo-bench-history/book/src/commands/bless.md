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
cargo bench-history unbless --local=./bench-history
```

## Effect on analysis

A blessing sets a new baseline for the selected benchmarks. It does **not** remove just one
data point: **all observations before the blessed commit are excluded from detection**.
The blessed commit itself remains eligible and starts the new baseline.

{{#include ../appendix/generated/reconstruction-blessing.svg}}

The shaded prefix is the history no longer judged, not a hole at the blessing marker.
The stored observations remain intact and visible in charts and [`examine`](examine.md);
changes within the new baseline can still be reported. The
[Reconstruction chapter](../appendix/reconstruction.md#blessings) explains how this boundary
applies to history and branch analysis.

## Rules

- `bless` takes one or more benchmark-id prefixes matched against the qualified identity, so
  it is deliberately per-benchmark — accepting the benchmark that caused trouble must not
  silently accept every other benchmark that may be trending badly. `--all` (mutually
  exclusive with prefixes) accepts every benchmark identity, including future identities.
- Both commands operate on a context ref (default `HEAD`), so any commit that resolves can be
  (un)blessed, not just the checked-out one.
- Blessing prefers — but does not require — the base branch and an existing clean run at the
  commit. Blessing **off the base branch warns**: it takes effect only once the commit joins
  the base branch's first-parent history (for example after a fast-forward), so a fast-forward
  merge workflow can bless a commit already on a feature branch. Blessing a commit with **no
  recorded run also warns** (double-check the commit id). The boundary still applies when
  that partition has no measurement at the anchor: the first measurement at or after it
  begins the accepted baseline. An unresolvable context ref, an undeterminable base branch,
  or no prefixes without `--all` are hard errors.
- A dirty working tree is allowed (the blessing targets the committed run) but warns.

## Logical scope

Choose the scope where expected behavior changes. With no discriminant options, `bless`
accepts the selected benchmark identities on **every engine, target and machine**. The
scope persists independently of measurements, covering partitions already present and
partitions discovered later. It never defaults to the machine executing the command.

For an intentional change limited to an engine, target or hardware class, specify
`--engine`, `--target-triple` or `--machine-key`. Each option is repeatable; values within
an axis are alternatives. Omitted axes and the value `all` are unrestricted. An investigation
performed on particular hardware does not alone justify a hardware restriction.

```console
# Accept a source-wide change for one benchmark family.
cargo bench-history bless --context <commit> package/benchmark/

# Accept an engine-specific change on every target and machine.
cargo bench-history bless --context <commit> --engine callgrind package/benchmark/
```

Discriminant scope is independent of `--all`, which removes benchmark identity restrictions,
not machine restrictions. Query commands retain their own current-machine defaults.

## Lifecycle

Blessings are immutable records. Repeated issuances coexist and apply together. Capturing or
overwriting a run never removes a blessing. Partition-local records remain readable and apply
only to their original partition, alongside logical-scope records.

`unbless` deletes the complete records at the context commit, not individual benchmark prefixes.
With no discriminants it removes all records there. Explicit discriminants restrict revocation:
every selected record must fit entirely inside the requested scope. A broader overlapping record
causes an error before deletion; the command never removes acceptance outside that scope or
silently invents exceptions. To narrow acceptance, revoke its complete scope and re-bless the
subset to keep. Blessings at other commits are unaffected.

History mode starts detection at the blessed commit while retaining earlier points for chart
context. Branch mode applies the blessing to the base ref's own first-parent evidence: the blessed
commit remains eligible and every earlier base observation is excluded from regime selection,
observed ranges, and historical report comparison. A recent blessing can therefore leave a branch
series unjudged until enough new base measurements accumulate.

Verify persisted intent with [`list blessings`](list.md):

```console
cargo bench-history list blessings --context <commit> --engine all --target-triple all --machine-key all --json blessings.json
```

The context view shows actual stored scopes, without shrinking them to the inspecting machine
or requiring measurements. In JSON, each empty array in `discriminant_scope` means unrestricted.
Check these axes, the anchor and the prefixes. `list blessings --all` instead reports effective
acceptance for measured series in the selected analysis window; it does not enumerate future
partitions or replace the stored-scope audit.

Logical-scope records require a tool version that supports them for analysis as well as writing.
Older readers of partition-local blessings do not apply project-level scope records.
