# Shape of the data

Before anything can be analyzed, it has to be measured, reduced to a common form, and stored.
This chapter is that form: what one benchmark produces, what survives into the store, and what
the store looks like.

It is the only place in this guide that documents the storage layer. If you are debugging why
two results are not being compared, or what a directory of JSON files means, this is the
chapter.

## Terms used here

{{#include generated/terms-shape.md}}

## One benchmark is not one number

The most common surprise: **a benchmark does not produce a timeseries. It produces
several — one per metric kind the engine reports.**

{{#include generated/shape-engine-series.md}}

So four benchmark identities in a suite can easily be eight or more series, and every one of
them is judged independently. That is why a store's series count is larger than the benchmark
count, and why the [false-discovery family](coverage.md) grows faster than you might expect.

## What each engine measures

{{#include generated/shape-engines.md}}

Everything is **lower-is-better**. There is no metric where a larger number is an improvement,
which is what lets the tool talk about regressions and improvements without knowing what a
benchmark does.

### What Callgrind deliberately throws away

Callgrind reports far more than three counts — cache hits at each level, estimated cycles,
branch mispredictions. Only the instruction count and the two branch counts are stored.

The discarded ones are simulated and highly sensitive to code layout: they can swing by tens of
percent between builds of *identical source*, because the simulator's model of the cache
depends on where functions landed in memory. Storing them would add series that regularly
report large moves for no reason — the precise failure this tool exists to avoid.

### Which number is taken

Each adapter picks one value per metric, and the choice matters more than it looks:

- **Criterion** prefers the regression slope over the mean where a slope is available. The
  slope is fitted across iteration counts and is much less sensitive to a slow first iteration.
- **`alloc_tracker`** and **`all_the_time`** take the per-iteration slope, never the recorded
  totals.
- **Callgrind** takes the new side of the pair it reports.

Two consequences worth knowing. The stored record does **not** say which estimator was used, so
a Criterion benchmark that stops producing a usable slope falls back to the mean silently — and
that can read as a step. And for the two operation engines, a missing or non-finite slope drops
the whole operation from the run rather than falling back to anything, so a benchmark can vanish
from a series without an error.

## Dispersion: what the engine tells you about its own precision

Some engines report a confidence interval alongside the value.

{{#include generated/shape-dispersion.md}}

The rule that matters is the same everywhere it appears in this appendix: **a confidence
interval can only ever take a candidate away.** No gate uses one to create or strengthen a
candidate. So an engine that reports none is not held to a weaker standard — it is judged on its
own between-commit scatter instead, which is arguably the more relevant quantity anyway.

`std_dev` is a special case: Criterion reports it, the tool stores it, and nothing reads it
back. See [Limits](limits.md#some-stored-facts-are-never-read-back).

## Benchmark identity

An identity is an ordered list of name segments, rendered with slashes. Each adapter builds it
from what its engine knows:

{{#include generated/shape-identity.md}}

Renaming a benchmark therefore starts a new series and retires the old one — the tool has no
way to know the two are related. See [Reconstruction](reconstruction.md#ghost-elimination).

Note the Criterion row carefully: **there is no package attribution**. Two identically-named
benchmarks in different crates of one workspace share a series. See
[Limits](limits.md#benchmark-identity-can-collide-and-the-estimator-is-invisible).

## What a stored run holds

One run is one engine's whole output at one commit, written as a single object.

{{#include generated/shape-run.md}}

The context is provenance. Only some of it drives behaviour:

| Field | Role |
|---|---|
| git commit | **Drives everything.** It is how a run is placed in the timeline. |
| target triple, machine key | **Partitions.** Part of the discriminant set. |
| environment provider, toolchain, tool version | Metadata. A change shows up as a step, deliberately. |
| observation timestamp | Provenance only. It never orders anything. |
| machine info, `best_of` | Recorded, never read back. |

That last row is not an oversight — see [Limits](limits.md#some-stored-facts-are-never-read-back).

The observation timestamp deserves emphasis: **when a benchmark was measured has no bearing on
where it sits in a series.** Position comes from git topology, read at analysis time. A commit
benchmarked out of order, or backfilled months later, lands exactly where its commit sits.

## Where it all lives

A storage key is a path with a fixed grammar:

{{#include generated/shape-key-grammar.md}}

A commit directory holds three kinds of object, distinguished by file name:

{{#include generated/shape-object-kinds.md}}

Each path segment is **sanitized**: characters outside a safe set are replaced, and the result
is lowercased. That is what keeps a key usable as both a filesystem path and a blob name — but
it also means two identifiers differing only in case or punctuation collapse into **one
partition**. Worth knowing if you set machine keys by hand.

Bodies are compressed at the storage boundary, so a local store is a tree of gzipped JSON.

## Atomicity, and what it does not cover

One stored object is written whole or not at all, and never rewritten: normal collection is
**append-only**. That is what keeps caches valid and history trustworthy.

The unit of atomicity is **one engine's output at one commit** — not the whole collection. Engines
are stored one at a time, so a run killed part-way through can leave a commit holding Criterion
data but not Callgrind. Nothing is corrupt, but the commit is incomplete, and a later
`backfill` will consider it already covered.

If a collection was interrupted, re-collect that commit with `--overwrite` rather than assuming
backfill will notice.

## Taking the minimum of several runs

`--best-of N` runs the whole suite `N` times and keeps, **per benchmark case and per metric
kind**, the smallest value.

Two details follow from "per metric kind":

- A stored result can mix metrics chosen from **different repetitions** — the instruction count
  from run 2, the branch count from run 3.
- The winning metric is kept whole, so its confidence interval travels with the value it
  belongs to. Intervals are never merged or recomputed.

The minimum is a useful heuristic here because shared-runner contention commonly adds delay.
Keeping the smallest observed value reduces the influence of transient positive interference,
without guaranteeing that the chosen value is disturbance-free.

> **`--best-of` is a protocol, not a flag.** The stored value depends on `N`, so changing it
> shifts every series at once and reads as a suite-wide step. See
> [Insights](insights.md#unreliable-benchmarks).

## What this stage hands on

A store: immutable objects, keyed by partition and commit, each holding one engine's reduced
output. Next: [Collection](collection.md), which is how they get there.
