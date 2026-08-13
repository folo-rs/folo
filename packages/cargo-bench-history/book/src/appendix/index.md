# Data pipeline

This appendix follows a single number all the way through the tool: from the moment a
benchmark engine writes it to a file, to the sentence in a report that says it moved.

The rest of this guide teaches the mental model. This part is the mechanism, in full, with
the numbers. It exists for two readers:

- **You have a finding that does not make sense**, and the concept chapters have not
  settled it. Chapter [Insights](insights.md) is the triage entry point; it links back into
  whichever stage you need.
- **You maintain this tool, or you have to trust it**, and you want to check that what it
  does matches what it claims. Every stage below states what it computes and against which
  threshold, so you can reproduce a verdict by hand.

## The five stages

<!-- The stage names used here are the ones every chapter title repeats, so a reader who
     remembers this diagram can navigate the rest of the appendix without the sidebar. -->

```mermaid
flowchart TD
    E["Benchmark engines<br/>criterion · callgrind · alloc_tracker · all_the_time"]
    E -->|"one file per engine"| C

    subgraph collection ["Collection"]
        C["collect / backfill"] --> S[("Stored runs<br/>one object per engine per commit")]
    end

    S --> SEL

    subgraph analysis ["Analysis"]
        SEL["Selection<br/>which objects are eligible"]
        SEL --> REC["Reconstruction<br/>runs become series"]
        REC --> DET["Detection<br/>did a level move?"]
        DET --> GAT["Noise gates<br/>is the move real?"]
        GAT --> FDR["Multiplicity control<br/>is it real given everything else tested?"]
    end

    FDR --> REP["Reporting<br/>findings, ranked, in three formats"]
```

Each stage preserves one thing, and naming those is the fastest way to understand the
shape of the whole:

| Stage | What it preserves |
|---|---|
| [Collection](collection.md) | Every stored run is a permanent, complete record of one engine's output at one commit. |
| [Selection](selection.md) | Only runs that are *comparable to each other* reach the analysis, decided without reading any of them. |
| [Reconstruction](reconstruction.md) | A series is ordered by git topology, never by when it was measured. |
| [Detection](detection.md) | A finding names a level that moved. It never claims to know why. |
| [Noise gates](gates.md) | A reported move is larger than what the measurement itself manufactures. |
| [Multiplicity control](coverage.md) | A reported move is unlikely to be an accident of how many things were tested. |
| [Reporting](reporting.md) | What was *not* judged is disclosed as prominently as what was. |

## How to read this appendix

The chapters are ordered along the pipeline, and each one assumes only the ones before it.
Reading them in order builds the whole picture; stopping anywhere leaves you with a correct
partial one.

Two conventions run throughout.

**Terms are defined before they are used.** Every chapter that introduces a statistical
term opens with a short list defining it in plain language. Terms also carry their
definition on hover — <abbr title="How often a value measured after a change beats one
measured before it, counted over every possible pairing.">like this</abbr> — and the
[Glossary](glossary.md) collects them all in one place. Where a plain description works as
well as the textbook name, this appendix uses the description.

**Nothing here is typed by hand.** Every number, chart, table and report excerpt is
generated from the same data the test suite asserts against, and regenerated whenever the
code changes. If a figure below shows a gate declining a move by a hair, that is what the
code actually did with that data — not an illustration of what it would do. This is also
why the examples are small: they are meant to be checkable, not realistic.

## Where this fits

| If you want | Read |
|---|---|
| To get the tool running | [Getting started](../getting-started.md) |
| To know what a command does | [Command reference](../commands/index.md) |
| The mental model, briefly | [Analysis](../concepts/analysis.md) |
| Why two results are or are not compared | [Comparability](../concepts/comparability.md) |
| To make your benchmarks less noisy | [Measurement stability](../concepts/stability.md) |
| The full mechanism, with numbers | this appendix |
