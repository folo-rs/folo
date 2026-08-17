# Collection

Collection is how measurements become history. Two commands do it: `collect` measures the
commit you are on, and `backfill` reconstructs a range of commits you never measured.

This chapter is what each actually does, and — more usefully — what shapes the store ends up
with.

## Terms used here

{{#include generated/terms-collection.md}}

## What `collect` does

```mermaid
flowchart TD
    P["Probe the environment<br/>git commit · dirty? · toolchain · machine key"]
    P --> R["Run cargo bench<br/>once, or N times under --best-of"]
    R --> H["Harvest each engine's output directory<br/>ignoring anything older than this run"]
    H --> D["Reduce to the common form<br/>one value per benchmark, per metric"]
    D --> S["Store one object per engine"]
```

The tool does not run engines by name. It enables the environment all four need and harvests
whichever produced output, so a workspace with only Criterion benchmarks simply yields
Criterion data.

### The harvest is freshness-gated

Engines write into `target/`, and `target/` persists between runs. Without a check, a collection
would happily re-store last week's output for a benchmark that failed to run this time — and
that stale value would enter history as if it had been measured at the current commit.

So harvested files must be newer than the run that was supposed to produce them. A benchmark
that did not run this time contributes nothing, rather than contributing a lie.

The consequence is worth stating: **an engine that produced no output stores nothing, silently.**
There is no error, because "this workspace has no Callgrind benchmarks" and "the Callgrind
benchmarks all failed" look identical from here. Coverage can expose missing output only for
series already present in history, where earlier observations can reconstruct as ghosts or
unjudged series. A never-recorded engine is invisible to coverage because collection has no
object, no series, and no way to distinguish "no benchmarks" from "a failed engine". If every
expected engine must be verified, that is a separate collection-time check, not an analysis
coverage guarantee.

## Collisions: what happens when a run already exists

Normal collection is **append-only**. Storing over an existing clean run is refused by default.

{{#include generated/collection-conflicts.md}}

*Why refuse by default:* history is only trustworthy if a recorded measurement stays put.
Silently replacing one would make a series depend on how many times someone re-ran a command.

## Dirty runs

A measurement taken with uncommitted changes is stored separately, under a key that includes
the observation time rather than replacing the commit's clean run.

That keeps three things true at once:

- the clean run at that commit is untouched;
- several dirty snapshots on the same commit coexist;
- the analysis can tell them apart and apply [its admission rules](selection.md#dirty-admission).

Only two dirty snapshots taken in the same second collide.

Dirty runs are what make the tool useful before you commit — you can measure a change in
progress — and they are also what makes uncommitted work on the base branch
[analyze as a branch](selection.md#mode-is-derived-not-chosen).

## Target triples and machine keys

The **target triple** comes from the toolchain that compiled the benchmark.

The **machine key** is a short hash over the host's hardware identity. What goes into it is
chosen deliberately:

{{#include generated/collection-machine-key.md}}

The exclusions are the interesting part. Clock speeds are excluded because they vary with
thermal state and power policy on the same physical machine — including them would rotate the
key spontaneously and shatter the history. Hostname is excluded so a renamed machine keeps its
history.

Verbose collection logs include the resolved machine key and fingerprint components, which is
the fastest way to diagnose a pool that is rotating keys unexpectedly.

## How runs land on commits

One commit directory per commit, per partition. There is no requirement that commits be
measured in order, or that every commit be measured at all.

{{#include generated/collection-occupancy.svg}}

Read that picture carefully, because most collection problems are visible in it:

- **Gaps** are commits nobody measured. Ordinary and expected — but the detectors
  [cannot see how wide they are](reconstruction.md#gaps-are-holes-not-zeroes).
- **A row that starts late** is a machine that joined the pool later.
- **A row that stops** is a benchmark that was deleted, or a machine that left.
- **Two half-filled rows** are the signature of a rotating CI pool, and the usual cause of
  [comparison-base lag](reporting.md#comparison-base-lag).

## `backfill`

`backfill` measures a range of commits you never measured, by checking each one out in an
isolated worktree and running the suite there.

It differs from `collect` in ways that matter:

- It plans over a **first-parent range**, newest first, so the most useful history arrives
  first if you stop it early.
- It **skips commits already covered** in the partition it is about to write, so re-running it
  is cheap and safe.
- It probes the toolchain **inside the worktree**, so a commit that pinned a different compiler
  is measured with that compiler.
- It works in a detached worktree, so your working tree is untouched.

### Filling the gaps a runner pool leaves

A rotating CI pool produces the two-half-filled-rows shape above: each machine key has data
only from the commits that happened to land on it.

This is the case backfill exists for, and the key point is that **another machine's data is not
coverage for yours**. Counts and timings are never compared across machine keys, so a
partition with holes stays a partition with holes until that machine fills them.

The fix is to run `backfill` on each machine over the shared range. Each fills its own
partition; each ends up with a history dense enough to judge.

## What can go wrong, and how it looks

| Symptom | Cause |
|---|---|
| A commit has some engines but not others | A collection was interrupted between engines. Re-collect with `--overwrite`. |
| A benchmark stops appearing | It was renamed or deleted; the old identity is now a [ghost](reconstruction.md#ghost-elimination). |
| A suite-wide step at one commit | Usually infrastructure or a changed `--best-of`, not code. See [Insights](insights.md). |
| Several machine keys where you expected one | The pool is rotating, or hardware changed; inspect verbose collection logs for the fingerprint components. |
| Everything reads `unknown` | Git reported no commit. Those runs can never match a real commit and are permanently ghosts. |

## What collection hands on

A store of immutable objects. Analysis reads it next, starting with
[Selection](selection.md) — which decides which of them are even eligible.
