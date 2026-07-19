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
8. **Lower-is-better, always.** Every persisted metric is normalized so a rise is a regression and
   a fall an improvement.
9. **Git topology orders time.** Series are ordered by first-parent committer-date topology read
   at analyze time, never by wall-clock measurement timestamps.

## Chapter map

The guide is three parts: **orientation** (get it working), **command reference** (task-level
how), and **concepts** (the mental model). Reference pages link down into concepts for the *why*;
concept pages link up into the commands that exercise them.

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
| `bless` / `unbless` | Manually accept an intentional change so history stops re-flagging it; per-benchmark; honoured only in history mode. |
| `machine-key` | Print the hardware fingerprint that hardware-dependent history is partitioned by; `--verbose` explains the factors. |

### Part 3 — Concepts

#### Benchmark engines

- **Goal**: the four engines and the two axes that drive the data model.
- **Teach**: hardware-dependent vs. -independent (drives partitioning) and confidence-interval vs.
  single-value (drives dispersion gating); *why no engine is deterministic* (tenet 2); what
  Callgrind deliberately persists vs. discards; the shared identity→metrics shape and
  lower-is-better (tenet 8).

#### Comparability and partitioning

- **Goal**: when two results are allowed to be compared.
- **Teach**: the central tenet — partition only by what makes results incomparable, record the
  rest as metadata (tenet 3); the discriminant set members; metadata-not-partition (toolchain,
  OS, commit, branch) so a rustc bump reads as a step; first-parent topology ordering (tenet 9);
  benchmark identity and what renaming does.

#### Analysis

- **Goal**: how findings are produced with high signal-to-noise.
- **Teach**: the two finding methods (change-point step vs. monotonic drift) and how one wins per
  series; the noise-aware gates and the practical-magnitude floor; the two auto-selected modes
  (history over the whole series vs. branch judging the *tip commit* against the base — tenet 7);
  full-history vs. bounded baseline-and-tip charts; re-baselining via resolved spikes and
  blessings; the three report formats sharing one pass and the advisory-finding /
  JSON-is-the-signal split (tenet 6); no severity classification.

## Maintenance

- When the CLI changes, the affected reference page *and* any tenet it illustrates move together;
  keep the "single thing it must teach" column honest.
- Prefer deleting a page's detail and linking to `--help` or a design doc over letting the guide
  drift from the code.
