# User guide skeleton

This document is the **blueprint** for the `cargo-bench-history` user guide (the mdBook under
`packages/cargo-bench-history/book/`). It is not the guide itself: it lists the chapters, the
job each chapter does, and the tenets each one must land. Edit *this* file to steer the shape of
the guide, then format the prose to match.

Keep it a skeleton. Bullet points and one-line intents only — no finished prose, no command
transcripts. When the guide and this file disagree on structure, this file wins; reconcile the
guide to it.

## Audience and scope

- **Reader**: an engineer who wants to catch performance regressions and long-range drift that
  snapshot-only benchmark tooling misses, and wire the tool into local work and CI.
- **This guide owns**: the *why*, the *when*, and the task-level *how* — installing, choosing
  storage, collecting/backfilling history, reading analysis, and the mental model behind it.
- **This guide does not own**: the library API surface (→ docs.rs) or internal architecture and
  rationale (→ the in-repo design docs). Link out; never duplicate.
- **Voice**: lead with value and motivation, not a flag dump. Every reference page states *what
  problem the command solves* before *how to invoke it*. Point at `--help` for the exhaustive,
  always-current flag list rather than transcribing it.

## Cross-cutting tenets

The teaching points that every chapter should reinforce, not just the concept chapters:

1. **History over snapshots.** A single run has nothing to compare against; the value is the
   reconstructed series. Motivate this before any command.
2. **High signal-to-noise, never cry wolf.** No engine is deterministic, so every metric is
   treated as noisy and gated hard. We would rather miss a marginal move than raise a false alarm.
   The gates weigh *evidence*, never cause, so a real-but-uninteresting shift is not filtered out
   for being uninteresting: an infrastructure-caused step is meant to be reported and then
   blessed. That is a statement about what the gates decline to consider, not a promise of
   detection — such a step still has to clear the same evidence and magnitude gates as any other.
3. **Comparability is explicit.** Two results compare only when their *discriminant sets* match;
   everything else is metadata so its effect shows up as a timeline step, not a silent fork.
4. **Append-only by default.** Normal collection never replaces an existing clean run; overwrite
   and prune are explicit maintenance actions. This keeps caches valid and history trustworthy.
5. **Storage is chosen at run time.** A shared config may describe Azure, while a local path comes
   only from a flag or environment variable and never enters the committed file.
6. **Findings are advisory; JSON is the signal.** The exit code reflects whether analysis *ran*,
   not what it found. Automation reads the machine-readable report.
7. **Prefer auto-detection.** Analysis mode is derived from git topology. Collection derives the
   machine key from hardware by default but allows an explicit stable key for a machine pool.
8. **Git topology orders time.** Series are ordered by first-parent committer-date topology read
   at analyze time, never by wall-clock measurement timestamps.

## Chapter map

The guide is four parts: **orientation** (get it working), **command reference** (task-level how),
**concepts** (the mental model), and an **appendix** (the full mechanism, for validation and deep
troubleshooting). Reference pages link down into concepts for the *why*; concept pages link up into
the commands that exercise them and down into the appendix for mechanism.

### Front matter — Introduction

- **Goal**: sell the problem in the first paragraph — trends visible only in hindsight against
  noisy history (incremental slowdown; a regression at commit Z seen only later).
- **Teach**: history-over-snapshots (tenet 1); the immutable-record + topology-reconstruction
  model at a glance; who the guide is for and what it defers to docs.rs / design docs.
- **Carry**: a one-diagram overview of producers → collect → storage → analyze → findings.

### Part 1 — Orientation

#### Installation

- **Goal**: get the binary present and runnable as a Cargo subcommand.
- **Teach**: `binstall` (prebuilt) vs. `cargo install` (from source); it surfaces as
  `cargo bench-history`.
- **Next steps**: point at `install`, storage, and first `collect`.

#### Getting started

- **Goal**: one end-to-end walkthrough on local storage, from nothing to a first analysis.
- **Teach**: the natural command order (install → backfill → collect → analyze → examine →
  machine-key) and *why a single collect is not enough* (tenet 1).
- **Carry**: link forward to comparability + analysis for the mechanism.

#### Storage backends

- **Goal**: explain where records live and how the backend is chosen.
- **Teach**: run-time selection (tenet 5); local vs. Azure Blob; the precedence ladder;
  `--no-store` as the exception; the read-through `--cache` for the cloud backend in CI; the
  shallow-clone / append-only CI notes.
- **Boundary**: describe the *selection model*; defer concrete Azure provisioning and config
  schema to the generated, commented starter file.

### Part 2 — Command reference

- **Overview page**: the command set at a glance; shared option groups; the selection-lockstep
  tenet (`analyze`/`list`/`prune`/`examine` share one selection pipeline); subjects are bare
  positional words, not flags. Point at `<command> --help` for the exhaustive flag list.
- Each command page: *problem it solves* → minimal invocation → the one or two rules that matter →
  links to the relevant concept.

| Page | The single thing it must teach |
|---|---|
| `install` | Generates a commented starter config; never clobbers; documents the optional Azure backend without storing a machine-local path. |
| `collect` | Harvests whichever engines produced output and persists immediately unless dry-running; `--best-of N` min-of-N noise reduction and its caveats. |
| `backfill` | Reconstructs history over a first-parent commit range in an isolated worktree; resumable and idempotent. |
| `analyze` | Reconstructs series from topology and reports regressions/drift; target/base/mode auto-selection; findings never set the exit code. |
| `examine` | Drill-down from a finding to the raw per-commit points of one `(benchmark, metric)` series; no detection, no judgment. |
| `list` | Preview the exact data set `analyze` would consume (`runs` / `discriminants` / `blessings`) without analyzing. |
| `prune` | Delete a chosen scope of stored data; never touches base-branch history without an explicit confirm. |
| `bless` / `unbless` | Manually accept an intentional change so history stops re-flagging it; per-benchmark; honored only in history mode. |
| `machine-key` | Print the hardware fingerprint that all history is partitioned by; it hashes hardware identity only, and `--verbose` explains the factors. |

### Part 3 — Concepts

#### Benchmark engines

- **Goal**: the four engines and the axis that drives the data model.
- **Teach**: every engine is machine-keyed (so partitioning is uniform) and confidence-interval vs.
  single-value (drives dispersion gating); *why no engine is deterministic* (tenet 2); what
  Callgrind deliberately persists vs. discards; the shared identity→metrics shape and
  lower-is-better.

#### Comparability and partitioning

- **Goal**: when two results are allowed to be compared.
- **Teach**: the central tenet — partition only by what makes results incomparable, record the
  rest as metadata (tenet 3); the discriminant set members; metadata-not-partition (toolchain,
  OS, commit, branch) so a rustc bump reads as a step; first-parent topology ordering (tenet 8);
  benchmark identity and what renaming does.

#### Analysis

- **Goal**: how findings are produced with high signal-to-noise.
- **Teach**: the two finding methods (change-point step vs. monotonic drift) and how one wins per
  series; the noise-aware gates and the practical-magnitude floor (relative *and* absolute, on
  every metric) and what they are *not* for (tenet 2); how branch mode judges the *tip commit* as
  one new observation against the base's commit-to-commit scatter; that confidence reports
  evidence strength; the false-discovery family being every judged series in either mode; the two
  auto-selected modes (tenet 7); full-history vs. bounded baseline-and-tip charts; re-baselining
  via blessings; the three report formats sharing one pass and the
  advisory-finding / JSON-is-the-signal split (tenet 6); no severity classification.
- **Boundary**: this page owns the *mental model* and stops there. Mechanism with numbers — which
  gate computes what, against which threshold, in what order — belongs to the appendix. Where the
  two would overlap, this page states the rule in a sentence and links down.

### Part 4 — Appendix: Data pipeline

A reference-grade walkthrough of the whole path from a benchmark's output file to a sentence in a
report. Where Part 3 builds the mental model, the appendix is what a maintainer validates the tool
against and what a user reads when a finding does not make sense.

- **Audience**: a maintainer checking the tool does the right thing, and the user who has exhausted
  the concept chapters. Not the first thing anyone reads.
- **Voice**: plain language first, technical name second. The reader is an engineer, not a
  statistician: every term is defined before use and again on hover, and jargon that buys the reader
  nothing is simply not used.
- **Evidence discipline**: every number, chart, table and report excerpt in this part is generated
  by `cargo-bench-history-figures` from data the test suite also asserts against, and included with
  `{{#include}}`. Nothing here is typed by hand. A regeneration that changes a figure means the
  pipeline's behaviour changed; the diff is the review.
- **Illustration discipline**: every stage that adds, removes, reorders, collapses or reshapes data
  gets a before/after figure in which affected observations are marked and labelled with the reason.
  A reader must never have to infer what a stage did.

| Page | The single thing it must teach |
|---|---|
| Index | The stages and the invariant each preserves; how to read the part; which chapter answers which question. |
| 1. Shape of the data | What one benchmark produces per engine, and what the stored record holds — including the storage layer the rest of the guide never mentions. |
| 2. Collection | What `collect` and `backfill` actually do, and how runs land on commits — including the gaps a heterogeneous runner pool leaves. |
| 3. Selection | Which stored objects are even eligible, decided from keys and topology alone; facets, `--since`, base/context, and how mode is auto-detected. |
| 4. Reconstruction | How runs fold into series, and the four things that change what detection sees: ordering, gaps, ghosts, blessings. |
| 5. Detection | What a signal is, which detector establishes it, and what each mode does and does not do. |
| 6. Noise gates | Every gate, in application order, with its computation and its threshold — and that gates short-circuit. |
| 7. Multiplicity and coverage | Why a per-series test is not enough; what the false-discovery family is and why it includes series that raised nothing; what the report means by *judged*. |
| 8. Reporting | Ranking, the three formats plus the lossy summary, charts, comparison-base lag, and why findings never fail a build. |
| 9. Insights | Triage playbooks: what to do with each kind of finding, and what to do when an expected finding never arrives. |
| 10. Limits | What the pipeline deliberately does not do, and what to do instead. |
| Glossary | Every term the part defines, in plain language, with the technical name alongside. |
| Reference tables | The generated lookup surface: metric kinds, engine outputs, key grammar, gate constants, evidence minimums, unjudged reasons, coverage states, JSON fields. |

## Maintenance

- When the CLI changes, the affected reference page *and* any tenet it illustrates move together;
  keep the "single thing it must teach" column honest.
- Prefer deleting a page's detail and linking to `--help` or a design doc over letting the guide
  drift from the code.
- The appendix's numbers are generated, so they cannot be edited in place: change the code or the
  example data and re-run `just book-figures`. A stale checked-in asset fails the test suite, so the
  appendix cannot silently fall out of step with the tool.
- When the appendix gains coverage of something a concept page also explains, trim the concept page
  to the mental model and link down. The two must not both carry the mechanism.


