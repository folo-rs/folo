# Reusable GitHub Action — design (issue #284)

This is the design for packaging the `cargo-bench-history` *collect → analyze → report*
flow as a reusable, Marketplace-published GitHub Action — a sub-component of the tool,
referenced from [`DESIGN.md`](DESIGN.md). It describes the intended end state of that
action: its command surface, distribution model, configuration surface, and hosting. The
tool's own data model, commands, storage, auth, and analysis modes live in `DESIGN.md`;
this document does not restate them, only how they are wrapped for external consumers.

**Reading guide.** For a refresher, read sections 1–2 for the purpose and ownership,
section 4 for the history/PR/backfill flows, and sections 5–6 for reporting and credentials.
[Section 12](#12-integration-and-deployment) describes integration contracts and maintainer
setup. Every section describes the complete intended design.

The components have distinct roles:

| Component | Responsibility | Focused reference |
| --- | --- | --- |
| `cargo-bench-history` | Measure, store, compare and render history; provision Azure without GitHub API access | [Application design](DESIGN.md) |
| `cargo-bench-history-github` | Reconcile workflow evidence and manage GitHub issues/comments around tool-rendered reports | [Companion design](../../cargo-bench-history-github/docs/design.md) |
| Composite action and reusable workflows | Give other repositories a small, standardized way to invoke those binaries | This document |
| Production Azure infrastructure | Store durable history using one federated managed identity | [Azure setup](DESIGN.md#710-setup-azure) |

An **instance** is the internal workflow and report namespace derived from the tool's
canonical storage project identity (§5). It is carried in companion arguments, job identities
and collection evidence, not supplied as a separate consumer setting.

## 1. Problem & goals

Folo's benchmark automation follows these flows, using the committed
`.cargo/bench_history.toml` for its history configuration:

* [`bench-history.yml`](../../../.github/workflows/bench-history.yml) collects the
  workspace's benchmarks into Azure on **every push to `main`**, analyzes the accumulated
  history for trend regressions, and files a rolling **issue**.
* [`pr-bench-history.yml`](../../../.github/workflows/pr-bench-history.yml) runs the same
  measure-and-report loop **on every pull request**, retuned to ask "does this PR move any
  benchmark relative to `main`?" — collecting only the touched packages, analyzing the PR
  head against `main` in the tool's **branch mode**, and posting a rolling **PR comment**.
* [`bench-history-backfill.yml`](../../../.github/workflows/bench-history-backfill.yml) runs
  **nightly** and replays collection across a window of recent history, densifying each
  machine key's series so comparisons have enough neighbouring points to judge against. It
  has **no analysis phase and no sink** — the next push-triggered run analyzes whatever it
  managed to fill in.

The tool is generic, so the same flows should be consumable by **any** repository
without checking out our workspace or copying our `just` recipes. **The goal is a published,
versioned Action** in `folo-rs/cargo-bench-history-action` that
generalizes the per-push history flow, the per-PR branch flow, and the nightly densification
pass into a small set of **parameterized commands**, takes its configuration from a
**committed `bench_history.toml` (or a `config` override)** rather than our `just` recipes,
and is listed on the GitHub Actions Marketplace. A consumer either calls the ready-made
**reusable workflows** that wire those commands into the standard job graph (§4.7), or
composes their own workflow YAML from the individual commands — instead of forking Folo's
hardcoded, repo-specific workflows.

**A load-bearing goal: trigger-agnostic, conflict-free.** The action must be usable from a
`schedule`, from `push` / `workflow_dispatch`, from `pull_request`, or from **several of
these at once in the same repo, without the invocations conflicting**. A repo may run a
scheduled collection *and* collect on every push to `main`; both can target the same commit.
The action therefore must not hard-code a single write-collision policy — see §4.5, where
the collect write mode is a caller-selected input, because no single policy fits both a
deliberately re-measuring run and an idempotent, cache-friendly collector. (The tool's own
bare default is a *hard error* on a duplicate commit — write-once immutability — and the
action keeps that same default, so a trigger-agnostic setup opts into `skip` explicitly, as
the README matrix does.)

**Both report sinks are first-class.** The per-push history view lands in a rolling
**issue**; the per-PR branch view lands in a rolling **PR comment** with its own lifecycle
(a while-you-wait placeholder, a staleness banner, and terminal states). These are genuinely
different reporting shapes, so the action exposes each as its **own command** (§4) rather
than a single analyze with a mode switch — the *kind* of analysis is inferred by the tool
from git topology (§4.5), while the *sink and its lifecycle* are what the caller selects.

Further goals shape *how much* the consumer has to own, and where the logic lives.
They are stated here because they cut across every later section:

* **Minimal consumer surface.** Adopting the flow must cost a consumer a handful of lines,
  not a complete job graph. The matrix, artifact handoff, concurrency groups, same-repo gate
  and sink lifecycle belong in shared orchestration, with repository choices expressed as
  inputs. The action therefore ships the whole job graph as **reusable workflows**
  layered over the composite action (§4.7), so the common case is a `uses:` line and a few
  inputs, and hand-assembly from the individual commands stays available for repos that need
  a different graph.
* **Logic belongs in Rust, not in shell.** Shell (PowerShell or otherwise) is the hardest
  layer in this system to test and the easiest to let drift from the tool it wraps.
  Duplicating report vocabulary and message composition there creates another implementation
  to keep consistent with the Rust analysis model. Every piece of behaviour
  that *can* live in a Rust binary should (§5.1), leaving the action's YAML as thin,
  near-logicless wiring. This is a testability goal first and a correctness goal second: a
  formatter that lives beside the data model it renders cannot drift from it.
* **Standard reports.** Consumers should not each invent their own comment and
  issue wording. The action posts **one standard, tested set of messages** (§5.2) so every
  consuming repo produces recognisably the same report. Reporting policy is fixed rather
  than expanded with cosmetic overrides.

Non-goals (explicitly out of scope here):

* **No new analysis behaviour.** The action is a packaging layer; it changes how the tool
  is *invoked and distributed*, not what it computes. The push/PR distinction, branch mode,
  ghost exclusion, best-of noise reduction, and the machine-key model all already exist in
  the tool and its workflows — the action surfaces them, it does not invent them. (Moving
  *report composition* into the tool, §5.1, is a presentation change, not an analysis one:
  the findings are identical, only who renders them moves.)
* **Publishing the tool itself.** The action installs a released `cargo-bench-history` on
  demand, so it depends on the package being available from crates.io — with `binstall`-able
  prebuilt binaries for the fast path (see
  [`../../../docs/release-automation.md`](../../../docs/release-automation.md)). *How* the tool
  gets published is out of scope here: this design consumes a published tool, it does not
  define the release pipeline.

## 2. Where the action lives — a dedicated repository

**Decision: a dedicated repository `folo-rs/cargo-bench-history-action`, with
`action.yml` at its root.**

The Marketplace requires the action's metadata file to sit at the **repository
root**, and a Marketplace listing is tied to a whole repository's release/tag
stream. The monorepo already spends its tag namespace on automated `release-plz` package
releases and per-binary-package GitHub Releases (`cargo-bench-history-vX.Y.Z`, etc.; see
[`../../../docs/release-automation.md`](../../../docs/release-automation.md)), so it cannot
also carry the clean, independently-moving `v2` / `vX.Y.Z` action tags the Marketplace and
the floating-major convention expect. A dedicated repo gives the action its own semver
stream, its own README/Marketplace page, and a `uses:
folo-rs/cargo-bench-history-action@v2` reference that does not drag in the monorepo.

*Rejected — an in-monorepo sub-path action* (`folo-rs/folo/.github/actions/
bench-history@<ref>`). Sub-path actions work for `uses:` but **cannot be published
to the Marketplace** (root-only requirement) and would have to be versioned by
monorepo-wide tags that collide with the release automation. We keep one *internal* thin
composite action in the monorepo only if it helps us dogfood (see §10), but the
**published** artifact is the dedicated repo.

*Where the code lives.* The split is by *language and release cadence*, not by convenience:

* **All Rust stays in the monorepo.** The tool, the companion binary (§5.1), and the faker are
  workspace packages, sharing its toolchain, lints, test conventions, release automation, and
  version groups. They are published to crates.io like any other package here and installed by
  the action at run time (§3). Nothing Rust is authored in the action repo.
* **Only the GitHub-shaped files live in the action repo**: the root `action.yml`, the reusable
  workflows (§4.7), their installation bootstrap (§3), the README, and the release tooling
  (§8.1). These are exactly the artefacts
  that need their own Marketplace listing and their own semver tag stream.

The action repo owns installation, invocation and caller-workflow checks, but no Rust
workspace. Source-installation tests still need a Rust toolchain. The monorepo owns the
substantive Rust behavior and its unit tests. The interface between them is the published
binary plus its documented arguments.

*Testability.* Most of the action's value lives in behaviour that is awkward to
test — a history that only becomes interesting once it accrues over many commits, and real
GitHub side effects (filing an issue, posting and updating a PR comment) — so it leans on a
layered strategy (§9): fast Rust unit tests of the composition and transport (in the monorepo,
beside the code), a local-storage
end-to-end pass on every push, and a synthetic-history pass (built on the published
`cargo-bench-history-faker` engine and the tool's hidden `import` command) that drives the real
GitHub-write paths.

## 3. Binary distribution

External callers cannot `cargo run -p cargo-bench-history` against our workspace, so the
action must obtain real binaries. The published tool is the `cargo-bench-history` package,
published from CI to crates.io.
Every published release also carries **prebuilt, `cargo-binstall`-consumable binaries**
(per-target archives + `.sha256`, attached to the release; see
[`../../../docs/release-automation.md`](../../../docs/release-automation.md)), so the action
has both a from-source path and a download-a-binary fast path.

The supported native collection targets are x64 Linux, x64 Windows and Apple Silicon macOS.
The reusable workflows select Linux and Windows by default; callers add `macos-latest` through
`platforms` when their benchmarks support it. The action manifest declares the supported
runner/target pairs, each covered by source canaries and the required published-installation
gate. The monorepo release process supplies their prebuilt archives. This support policy is
independent of which platforms Folo selects for its own performance measurements.

The action exposes an **`install-method`** input. **The chosen method applies
to every binary the action needs**, not just the tool: the companion (§5.1), and any other
`folo-rs/folo` binary a command depends on, are obtained the same way. A consumer who chose
`binstall` must not have that choice silently ignored for the second binary. Each command
installs only the binaries it actually uses.

| `install-method` | How | When |
| --- | --- | --- |
| `binstall` | Install `cargo-binstall`, then `cargo binstall <package> --version =<v> --locked --no-confirm` for each required binary | **Default.** Downloads the prebuilt archive from each package's GitHub Release, falling back to a source build if no asset matches the runner target. Best for `cargo-bench-history`, whose Azure SDK + `mimalloc` dependencies are slow to compile. |
| `install` | `cargo install <package> --version =<v> --locked` for each required binary | Pure source build of the published package, requiring its build prerequisites and paying the cold-cache compile cost. The escape hatch when a prebuilt asset is unavailable or unwanted. |
| `path` | `cargo install --path <source-path>/packages/<package> --locked` for each required binary | Dogfooding (§10): one Folo source checkout supplies every required binary. |

**Bootstrap runs before Rust is available.** One thin, action-owned PowerShell bootstrap
selects `binstall`, `install` or `path`, reads the release manifest, and obtains the required
binaries. Released methods use the exact manifest pins; `path` builds the selected checkout.
It handles the installed-binary cache and process invocation, not report interpretation or
workflow evidence. Composite commands and reusable-workflow preparation use the same bootstrap.
PowerShell is necessary at this boundary because the selected Rust binaries must be installed
before they can run; substantive post-installation logic belongs in Rust (§5.1).

**The action version selects the binary versions.** For `binstall` and `install`, every
binary version comes from the action release's manifest. There is no caller version
override or independently moving latest-tool selection. Updating the tested combination is
an action release, which may change only that manifest. Tool and action version numbers
remain independent. Every monorepo PR moving a pinned tool version has a paired action PR
to adopt it, following the [release policy](../../../docs/benchmark-action-releases.md).

A pinned action release or commit selects an exact tested combination. Each floating major
tag advances only among that major's action releases with their own tested manifests.
`path` deliberately builds unreleased code from the supplied Folo checkout.

**`binstall` — the fast default.** `cargo-binstall` resolves the package's GitHub Release
and unpacks the binary, with source installation as its fallback. Installation remains
subject to network, toolchain and target availability. Published `.sha256` sidecars support
explicit verification; `cargo-binstall` does not automatically discover them
([release-asset contract](../../../docs/release-automation.md#the-asset-naming-contract)).
`--locked` uses the published `Cargo.lock` for reproducibility.

Released installations cache resolved binaries with `actions/cache` keyed by `(runner.os,
runner.arch, manifest versions)`; a cache hit avoids reinstalling the same versions, including
repeating a source-fallback compile. Cache availability is not guaranteed.
`path` does not restore this installed-binary cache: it builds the selected source checkout,
so a released manifest cannot substitute a binary from another revision.

**One manifest for the tested tool set.** A sink-using flow needs the tool *and*
the companion (§5.1).
The **release manifest** records the action's own release version and exact versions of every
monorepo binary used by the action, workflows or their tests. These are separate version
selections, not a promise that the binaries have the same package version:

* The **tool** is the public dependency, pinned to its tested version.
* The **companion** is an action-internal implementation detail with no stable CLI, so it is
  **pinned by the action release** to the version tested with that tool.
* The **faker** is independently pinned for the action's tests. Its version is not inferred
  from the tool or companion version.

The companion links `cargo-detect-package` as a Rust library for workflow scope selection (§4.7).
It follows the companion's ordinary Cargo dependency/version plan; the action neither installs
a detector executable nor carries a separate detector pin.

Workspace release groups are derived from exact first-party dependency requirements
([release versioning](../../../docs/release-versioning.md#version-groups)). The tool and its
`cbh_*` implementation packages form such a group; the companion and faker are independent
packages. The companion reuses core configuration and canonical project-key resolution through
compatible dependencies, without exact requirements that would join the core's version group.
No dependency exists merely to align version numbers. The action manifest records a tested
combination across those independent releases.

**Released installation uses published binaries.** The required installation gate checks
the pinned tool and companion versions through their supported installation methods before
an action release. Source dogfooding with `path` does not prove the released installation path;
the required gate (§8.1) exercises that path independently. New crates use the first-publication
process in [`RELEASING.md`](../../../RELEASING.md).

Every composite command uses the companion's action execution boundary for validated inputs,
project identity and workflow outputs. `collect`, `backfill`, and `analyze-*` additionally
use the main tool. Publication and lifecycle commands do not install the main executable.
`path` accepts one `source-path`, the root of the Folo source checkout.

**Failure reporting depends on installation too.** `alert` (§4.4) runs because something
already went wrong. The ordinary binary cache and bounded installation retries reduce its
exposure to transient faults; they are not special mechanisms for `alert`. The release gate
(§8.1) verifies the manifest's binaries on supported targets before moving the floating major
tag, not their availability on every later runner. Bootstrap, installation or required
companion-artifact failure can leave the companion unavailable. In that case no alert is
published and the workflow remains failed (§12); there is no second publisher.

**All install modes stay testable.** The PowerShell bootstrap is tested directly for method
selection, manifest pins, process arguments and cache decisions with mocked tool output.
Post-installation Rust logic has its own fake-driven tests (§5.1).
The CI matrix (§9) additionally runs each *real* method (`binstall`,
`install`, and `path`) against the corresponding release or source checkout, so
both the branching and the actual installs stay covered. Actual-method canaries bypass the
installed-binary cache and use isolated installation roots, just like the release gate (§8.1).

**No test scaffolding is ever shipped to consumers.** `cargo install cargo-bench-history` (or a
`binstall` of it) installs **only** the real tool. The end-to-end test engine is a *separate
package*, `cargo-bench-history-faker`, with its own binary; installing the tool never pulls it
in (see `DESIGN.md` §9). The faker is **published but unsupported**, with
`binstall`-able prebuilt binaries from the same release pipeline: its
crate root is doc-hidden and neither its library API nor its CLI carries a semver contract. It
is published purely so a test job can run it without vendoring or a workspace checkout (§9).
Released installation tests require every selected package to be published. A consumer of the
action never installs it: it appears in the release manifest only for
the action's own test jobs. So the action needs no `--bin`
selector or any other guard against test binaries leaking onto a consumer's `PATH`; the install
commands above are the plain package-name form.

## 4. Action shape — one root action, a `command` selector

**Decision: a single composite action at the repo root with a required `command`
input**, one value per pipeline stage of the supported flows. Collection, analysis, publication
and lifecycle operations are independently schedulable:

| `command` | Role |
| --- | --- |
| `collect` | Measure and store (per platform, in a matrix). |
| `backfill` | Densify recent history for this machine key; no analysis, no sink (§4.8). |
| `analyze-history` | Trend analysis of the selected history branch, emitting reports once after the matrix. |
| `analyze-pr` | Branch-vs-base analysis of a PR, emitting reports once after the matrix. |
| `publish-comment-findings` | Publish PR findings, retaining any incomplete-coverage qualification. |
| `publish-comment-clean` | Publish a fully covered clean PR result. |
| `publish-comment-preflight` | Mark prior PR results stale or seed an in-progress placeholder. |
| `publish-comment-inconclusive` | Explain empty scope or a successful analysis without a complete verdict. |
| `publish-comment-failed` | Replace this run's unfinished PR placeholder with a failure/cancellation notice. |
| `publish-issue-findings` | Create or update the rolling regression issue from history findings. |
| `publish-issue-clean` | Move an existing regression issue to all-clear, leaving it open. |
| `publish-issue-preflight` | Mark an existing regression issue stale while another run is in progress. |
| `publish-issue-inconclusive` | Annotate an existing regression issue when this run cannot establish recovery. |
| `publish-issue-failed` | Annotate an existing regression issue when its pending run fails. |
| `alert` | File a one-off workflow-failure issue, separate from the rolling regression issue. |

A single monolithic "do everything" action cannot express these shapes: `collect` runs
*per platform in a matrix* while every `analyze-*` runs *once, after the matrix*, and the
report-sink lifecycle steps (`publish-comment-preflight`, `publish-comment-failed`, `alert`) run in
their own jobs at different points. A `command` selector keeps a single Marketplace listing
(only the root action is listed; sub-path actions are not) while letting each invocation play
one role. The split of `analyze` into `analyze-history` and `analyze-pr` is deliberate:
each carries a **cohesive, independently-validated input group** and feeds a **different report
sink**. Publication is a separate invocation after the workflow uploads the reports, so the
artifact link exists before the companion composes the message. The predefined workflows run
analysis, upload and publication as successive steps in one job; the analysis command itself
never posts.

Inputs that do not apply to the selected command are **rejected, not ignored**: the action
validates the combination up front (e.g. `command: collect` with an analysis-only `since`, or
`command: publish-comment-findings` without a PR number) and fails with a clear error. Silently
ignoring a misplaced input is how a caller ends up believing a sink is configured when nothing
will post.

These commands are the *building blocks*. Wiring them into the standard job graph is itself
boilerplate that no consumer should have to write, so the same repo also publishes reusable
workflows that do it — see §4.7.

*Rejected — a single `analyze` with a `mode`/`report-target` switch*: the tool already
infers history-vs-branch from git topology (§4.5), so a `mode` input would be redundant, and
folding the issue and PR-comment sinks (with their very different lifecycles) into one command
would produce one bloated, half-applicable input set. *Rejected — a single all-in-one
action*: cannot express matrix-collect + single-analyze + separate lifecycle jobs.

### 4.1 `collect`

1. **Install** the tool per `install-method` (§3).
2. **Collect** — one invocation, no surrounding logic:
   `cargo-bench-history collect [--config <config>] [--local=<path>] (--workspace [--exclude
   <pkg>…] | --package <pkg>…) [--bench <name>…] [--all-features] [--best-of <N>]
   [--overwrite | --skip-existing] --verbose`.
   * **Scope.** With no `packages` input the collect is workspace-wide (`--workspace`, minus
     any `exclude`), as the push flow runs it. With a `packages` input it collects **only
     those packages** (`--package` per name), as the PR flow runs it — the reusable workflow
     computes the affected, benchmarkable set (§4.7), or a composite caller supplies its own.
     `packages` and `exclude` conflict at this layer, matching the CLI; workflow preflight
     applies exclusions before passing an explicit package list. Analysis uses the
     context-presence filter rather than inferring benchmark identities from package names
     (§4.3).
   * **Noise reduction.** `best-of <N>` (default 1; the workflows pass 3) runs the suite N
     times per commit and keeps each metric's minimum sample — runner interference is
     one-sided, so the minimum is the reading least perturbed by transient load.
     The workflows preserve Folo's established repetition count so shared orchestration
     uses the same measurement protocol as its stored history, rather than introducing a
     systematic level shift. This is a compatibility choice, not a claim that the count is
     optimal for every repository; see the
     [measurement protocol](DESIGN.md#5-run-context) discussion.
   * **Write mode.** The write-collision policy is the caller's **`on-existing`** input, not
     a hard-coded flag (§4.5).
   * **`--config`** is passed **only when the `config` input is set**; otherwise the tool
     discovers the repo's committed `.cargo/bench_history.toml` (§5).
   * **`--verbose` is always enabled.** The tool's verbose channel is *explanatory* — it states
     the inputs and reasoning behind each decision rather than announcing conclusions — and a
     CI reader cannot re-run the job locally to find out why it did what it did. Paying for
     that reasoning in the job log on every run is therefore worth it: the log is the only
     forensic record a consumer has when a run measures nothing, skips a package, or picks an
     unexpected partition. The predefined workflows do not expose a verbosity toggle.
3. **Emit this leg's machine key.** After a *successful* collect, the action resolves this
   runner's real hardware fingerprint (`cargo-bench-history machine-key`) and exposes it as a
   `machine-key` **output**, so the caller can hand the exact keys measured this run to the
   later single analyze job (§4.6). A failed leg has no confirmed complete collection, so it
   emits no key; failure does not promise that no individual object reached storage.

**The collect matrix does not fail fast, and the reason is structural.** Each platform writes
into **its own machine-key partition**, and analysis never compares across partitions (§4.6).
A failed leg withholds its receipt and is disclosed as incomplete coverage. Previously stored
or partially written objects do not turn that failure into confirmed collection success.
Successful legs remain usable for qualified findings.

The objection is worth taking seriously: if a benchmark is broken, surely every leg fails, so
letting them all run just burns runner minutes. That is true of *systematic* failures — and
they are also the cheap case, because a benchmark that does not build fails early, and a run
where every leg failed publishes no analysis verdict because no collection completed. The
asymmetry is in the other class. **Platform-specific and environmental failures are real and observed**: a
runner image change, a toolchain install hitting a transient disk fault, a storage hiccup, or
a benchmark that only breaks on one OS. There, `fail-fast: true` would *cancel the surviving
legs mid-flight*, discarding hours of valid measurement and leaving a permanent hole in those
platforms' series for that commit — recoverable only by a later densification pass (§4.8),
which costs more than was saved. So the choice trades a bounded waste in the cheap case
against unrecoverable data loss in the expensive one.

Tolerating partial collection does carry one real hazard, and the design pays for it rather
than ignoring it: a report drawn from the surviving platforms can look like a broader clean
bill of health than it earned. That is why coverage is disclosed rather than assumed (§4.2) —
the two decisions are a package, and tolerating partial failure without the disclosure would
be the genuinely wrong design.

**Bad data is removed, not repaired by a special workflow.** A maintainer removes unsuitable
measurements with the tool's manual `prune` command. The resulting gap is acceptable.
Ordinary backfill may fill it if it falls within that flow's scope; targeted historical
recollection and other hole-filling automation are outside the action's scope.

### 4.2 `analyze-history` (→ rolling issue)

1. **Install** the tool per `install-method`.
2. **Verify full git history (validate, don't mutate).** `analyze` splits the target's
   first-parent ancestry at its merge-base with the base, so a shallow clone that stops short
   of the branch point makes the merge-base unresolvable — a **hard error** in the tool
   (`DESIGN.md` §7.3/§8.5). The action runs `git rev-parse --is-shallow-repository` up front
   and **fails fast** with an actionable message — "check out with `fetch-depth: 0`" — rather
   than surfacing the tool's later error or silently `git fetch --unshallow`-ing someone
   else's working tree (the idiomatic place to control depth is the caller's own
   `actions/checkout`).
3. **Choose a scratch directory *outside* the checkout.** All rendered artefacts — the
   reports, the condensed summary, and the `--cache` mirror — are written under a
   runner-temp scratch dir (`${RUNNER_TEMP}/bench-history`), never inside the working tree.
   This is load-bearing: `analyze` labels the analyzed tip with the repo's dirty state via
   `git status --porcelain`, so an artefact written into the checkout would make an
   otherwise-clean `main` look dirty and mis-annotate the tip.
4. **Analyze**:
   `cargo-bench-history analyze [--config <config>] [--local=<path> | --cache=<dir>]
   --context <commit> --base <commit> --no-dirty
   --engine all --target-triple all --machine-key <k>… --no-text --markdown <scratch>/report.md
   --json <scratch>/report.json --markdown-summary <scratch>/summary.md [--since <window>]
   --outcome <scratch>/outcome.txt --verbose`.
   * **Analysis mode is inferred, not passed.** The action resolves the collected commit and
     passes it as both `--context` and `--base`, excluding dirty snapshots. The tool then
     selects **history** mode — long-range
     change-point and drift detection reporting regressions only (§4.5). The action passes no
     mode flag because none exists. Merely checking out a release branch is insufficient:
     omitting `--base` would still resolve the tool's configured or detected default branch.
   * **Nothing here is specific to a branch named `main`.** History mode is selected by
     *topology*, not by a branch name: it applies to whatever branch the run collected on, so a
     repo whose trunk is `master`, `develop`, or `trunk` works unchanged, as does a repo running
     the flow on a long-lived release branch. What the flow does assume is that it runs on **one
     branch that accumulates a continuous series** — the trunk, in practice — because a history
     is only comparable along a line of commits that share ancestry. Running the history flow
     against a second branch is supported, but it is a second history: give it its own
     configured project ID (§5) so the two do not share a rolling issue, an artifact name,
     or a cache key.
   * **Machine keys.** The facets default to surveying every engine and triple (`all`), but
     the machine key is **not** `all`: it is the exact set of fingerprints collected this run,
     threaded from the collect matrix (§4.6), so the survey follows the data partitions
     selected by this action's collection, rather than every source that happened to measure
     the same commit. For example, a manual collection on another PC does not join merely
     because its commit matches. Machine keys identify comparable hardware, not provenance:
     other measurements under the same key remain part of that partition's history.
   * **Platform coverage is disclosed, not assumed.** Because the matrix tolerates a partially
     failed collect (§4.1), the **completed platforms** can be a subset of the **expected
     platforms**, and the difference is invisible in the findings themselves. Expected
     platforms come from the matrix input; completed platforms require successful latest
     collection jobs and matching receipts (§4.6). When they differ the report says which platforms
     are missing and carries a **partial-platform-coverage** qualifier beside the analysis
     outcome. On a fully successful run this adds nothing, so the common case stays quiet.
     Without it, a Windows leg dying
     would leave "no regressions" standing as an unqualified claim about a platform nobody
     measured — which is the failure mode that makes silent partial coverage worse than an
     outright failed run.
   * **Cache.** `--cache=<dir>` (a read-through mirror of the cloud history persisted across
     runs via `actions/cache`) turns repeated full-history downloads into a warm-cache read;
     it applies to the **cloud backend only** and **conflicts with `--local`**, so the action
     passes at most one.
   * A single pass emits the full Markdown and JSON reports, the **condensed top-findings
     Markdown summary** (`--markdown-summary`), and the named outcome file. Machine decisions
     use JSON or the outcome, not the human-readable Markdown.
5. **Surface the analysis outcome**, not merely a boolean. A successful analysis ends in one
   of `findings`, `clean`, `insufficient_baseline`, `nothing_in_scope` or `partial`, and that
   **`outcome`** is what the flow carries forward alongside the report paths and regression
   count (§7). A single `notable` boolean collapses states that need different handling:
   "clean" and "we could not judge anything" are both *not notable*, yet only one of them is
   good news. Naming the state serves two consumers. It is the key the companion selects a
   message with (§5.2), so the choice is made once and explicitly rather than re-derived from
   scattered checks; and it lets the test canaries (§9) assert what actually happened, where
   an assertion on `notable == false` would pass equally for a clean run and for a broken
   fixture that analyzed nothing. Callers may branch on it too, but that is a side benefit,
   not the justification. Execution failure and partial platform coverage stay separate
   workflow facts, because both can coexist with any analysis verdict.
6. **Publish after report upload.** The job uploads the full Markdown + JSON reports,
   the summary, and the outcome and collection-coverage metadata as an **artifact**, then
   invokes `publish-issue-findings` when publication is enabled and the outcome
   is **findings**, including when platform coverage is partial. It finds the open rolling
   issue through its stable project-qualified title (§5.1), then creates or updates the body
   with the tool-composed summary, any missing-platform qualification, and the artifact link.
   The job has Azure access and `issues: write`; no cross-job report handoff is required.
   A fully covered clean run routes to `publish-issue-clean`, leaving the issue open.
   Other successful verdicts route to `publish-issue-inconclusive`, preserving existing findings
   with an explanation rather than implying recovery (§4.4).

**Empty-run degeneracy.** When every collect leg failed there are no successful machine-key
artifacts to thread. The workflow skips analysis and records execution failure, not a
successful `clean` or `nothing_in_scope` verdict. Any diagnostic placeholder is explicitly
not an analysis report; notification belongs to the failure lifecycle (§4.4).

### 4.3 `analyze-pr` (→ rolling PR comment)

Structurally the same install → validate-history → scratch-outside-checkout → analyze →
outcome pipeline as `analyze-history`, retuned for the PR branch view. The same job uploads
its reports and invokes comment publication:

* **Checkout is the PR head's *real* commit, with full history** — not the synthetic
  `pull_request` merge ref, whose first parent is the base branch and would corrupt the
  comparison. The caller checks out `pull_request.head.sha` with `fetch-depth: 0` (which also
  populates the remote-tracking ref the merge-base needs).
* **Branch mode is inferred from an explicit base.** The action passes `--context HEAD --base
  <base>`, where the base is **the PR's own base ref** taken from the event — not a hardcoded
  branch name, so a repo whose trunk is not `main`, or a PR targeting a release branch, is
  compared against the right thing. Because the PR head is ahead of its merge-base with the
  base, the tool auto-selects **branch mode** — it judges the branch by its **tip commit**
  against the recent base level, discarding the branch's own intermediate history (only the tip
  lands on the base, so intermediate commits say nothing about the merge's effect) and
  comparing just the newest commit's runs, in both directions (`DESIGN.md` §8.5). `--base` is
  passed explicitly because automatic default-branch resolution does not identify a PR's
  actual target branch.
* **Base-lag is surfaced, not hidden.** Because every engine is now machine-keyed (§4.6) and
  the tool never compares across machine keys, a PR built on one runner of a rotating public
  pool may find usable base data only under *its* key — while the newest base commits ran on a
  *different* machine. Branch-mode findings therefore carry a per-result warning when the
  comparison base lags the merge-base (distinguishing "a newer base run exists but under a
  different machine key" from "no base data at more recent commits"), so a comment comparing the
  tip against a state several commits back says so instead of looking authoritative. The action
  needs nothing for this — it is inherent to branch-mode analysis — but it is why the machine-key
  handoff and the honest scope disclosure matter on GitHub's shared runners.
* **Both directions are reported, without a flag.** Branch mode always reports regressions
  *and* improvements; history mode always suppresses improvements. The direction filter is a
  property of the mode, applied *before* the false-discovery correction so the correction only
  ever sees candidates that mode would actually report. The action passes no direction flag.
* **Scoping falls out of collection, never a name filter.** Analysis is deliberately *not*
  package-scoped: benchmark identities are engine-dependent, so an id-prefix filter would
  silently drop some engines' series. Instead the tool's **always-on ghost exclusion** analyzes
  only benchmarks present at the context commit (the PR head) in the selected machine
  partitions. Collection determines that presence, including any existing measurements at
  the same commit and machine key. Ghost exclusion is a presence filter, not a provenance
  filter, and works for every engine without additional action parameters.
* **Cache is restore-only.** PR runs read the shared history cache but never save, keeping the
  baseline warm without accumulating per-PR cache entries (safe against the append-only store
  even when slightly stale).
* **Sink: a rolling PR comment.** After report upload, the appropriate
  `publish-comment-findings`, `publish-comment-clean` or `publish-comment-inconclusive` command posts
  the condensed summary as a single comment on the
  PR, deduped by a hidden marker and updated in place on every push, by the companion
  transport binary (§5.1). The
  comment is strictly advisory (findings never affect the check's exit code), reports
  improvements alongside regressions or a plain "no regressions" state, and **states its
  collection scope on both axes** — which packages were benchmarked *and*, when the collect
  matrix came back partial, which platforms are missing (§4.2) — so a clean result is never
  mistaken for the whole suite, or the whole runner pool, being clean. The PR flow runs the
  same `fail-fast: false` matrix as the history flow (§4.1), so it is exposed to exactly the
  same partial-coverage hazard, and this is the more widely read of the two sinks: a reviewer
  deciding whether a change is safe to merge must be able to see that a platform went
  unmeasured. Missing platforms qualify the comment without replacing the analysis outcome:
  `findings` still reports findings. A tool verdict of `clean` with missing platforms routes
  to `publish-comment-inconclusive`, retaining that limited clean result and its qualification
  rather than presenting a complete all-clear.
  The tool's `partial` outcome instead means some in-scope series went unjudged with no findings.
  It records **which commit it measured** (a bare full SHA
  that GitHub autolinks, plus a hidden full-SHA marker) so staleness can be judged later
  (§4.4). A run *failure* produces no fabricated report or new failure comment;
  `publish-comment-failed` only retires its own unfinished placeholder. The caller grants
  `pull-requests: write`.
* **Never presents already-stale results as fresh.** A run takes hours, so its results can be
  obsolete by the time it posts. Two guards cover the finish side. First, analysis and
  publication are gated
  on `!cancelled()` (not `always()`): a *superseded* run — cancelled by the next push's
  concurrency group (§8) — never reaches the post step, while a merely partially-failed collect
  still reports what landed. Second, for the narrow window where a new push *races* the final
  post faster than cancellation can stop it, comment publication **re-reads the live PR head just
  before posting** and, when it no longer matches the analyzed (frozen) SHA, injects the same
  staleness banner into the body *before* posting — so a superseded result never appears fresh.
  The check **fails closed**: if the live head cannot be read at all, the body is posted with a
  "freshness could not be verified" note rather than with an implicit claim of freshness. The
  degradation is in the *precision* of the banner (an exact commit distance may be
  unavailable), never in whether the reader is warned.

### 4.4 Report-sink lifecycle commands

Each rolling sink uses **`publish-<sink>-<state>`**, with `comment` or `issue` as the sink.
The action and companion use the same names. Commands describe the state being published,
not a vague maintenance operation. Their scheduling remains the workflow's responsibility.

**Publication state is derived from evidence, not a caller's preferred wording.** Analysis
and report inspection project one publication state from the successful report and platform
coverage. The workflow forwards the `publication-state` output as the sink command suffix:

| Evidence | State |
| --- | --- |
| Findings, even with missing platforms or unjudged series | `findings`, with coverage qualifications |
| Clean analysis, every in-scope series judged, every expected platform completed | `clean` |
| Successful analysis without findings but with insufficient baseline, empty scope, unjudged series or missing platforms | `inconclusive`, with the actual limited result and reason |
| Scope preflight selected no benchmarkable packages, so analysis did not run | `inconclusive`, explicitly saying nothing benchmarkable changed |
| Collection, analysis or publication failed or was cancelled | `failed`, never an analysis verdict |

`inconclusive` means **no complete clean verdict is available**, not necessarily zero samples.
Its message states the actual reason and preserves any useful partial report; it never says
that no measurements exist merely because some could not be judged. Failed execution cannot
be presented as `inconclusive`. Findings take precedence over incomplete coverage.
The core analysis outcome is unchanged; publication state chooses the sink operation rather
than replacing the analysis explanation.

Each report-bearing publication command asserts that its named state agrees with the supplied
evidence, then passes that checked state to publication and message composition. This assertion
does not silently reroute a mismatched command. The workflow does not repeat the state-selection
policy or infer it from Markdown. Explicit empty scope retains its separate flag and no-report
input form rather than inferring scope from missing evidence.

**PR-comment sink (branch flow).**

* **`publish-comment-preflight`** runs alongside collection for nonempty scope. It creates or
  refreshes a scope-disclosing in-progress placeholder when no completed report exists.
  Otherwise it retains the report and adds a replaceable staleness banner. GitHub's commit
  comparison supplies the distance when available; an unknown distance is disclosed rather
  than presented as fresh. The same banner logic qualifies finish-side publication (§4.3).
* **`publish-comment-findings` / `publish-comment-clean`** create or update the one rolling
  comment with the corresponding validated report. Findings include improvements in branch
  mode and remain visible when coverage is incomplete. A clean comment requires complete
  evidence; absence of findings alone is insufficient.
* **`publish-comment-inconclusive`** creates or updates the comment with the applicable explanation.
  Empty package scope produces the short nothing-benchmarkable-changed note, including when
  no comment exists. A successful but inconclusive analysis retains its tool-rendered summary
  and coverage explanation. A missing comment would make either case indistinguishable from
  broken or still-running automation, so neither path deletes the comment.
* **`publish-comment-failed`** is the terminal step after failure or cancellation. It replaces
  only an in-progress placeholder owned by this workflow run, attempt and frozen head, linking
  the run and distinguishing failure from cancellation. It creates no new comment and leaves
  completed results and a superseding run's placeholder untouched.

Preflight and empty-scope publication require the frozen head to match the live PR head.
Report publication carries the analyzed commit and checks freshness immediately before the
write, preserving any comment owned by the current head, including pending and terminal notes.
Attempt numbers order attempts only within
the same workflow run ID; a delayed earlier attempt cannot retire its rerun's placeholder.
Across distinct runs, commit-order and live-head guards apply. Distinct runs at the same
commit follow serialized publication order, not a comparison of their attempt numbers.

**Regression-issue sink (history flow).**

* **`publish-issue-findings`** is the only rolling-issue command that creates an issue. It
  publishes validated history findings and their coverage qualifications.
* **`publish-issue-clean`** rewrites an existing open issue to all-clear at the analyzed commit,
  but leaves it open. Recovery of the numbers does not establish that the underlying problem
  is understood; closing the investigation remains a maintainer decision.
* **`publish-issue-preflight`** marks an existing report stale and records the pending run,
  attempt and head. It creates no placeholder issue: an issue merely announcing a benchmark
  run would be tracker noise rather than useful context on an existing finding.
* **`publish-issue-inconclusive`** preserves the existing report and annotates why this run could
  not establish recovery, linking the available report. The retained report's analyzed commit
  and any preflight staleness qualification remain visible. Freshness qualification does not
  depend on preflight having run. Missing evidence never becomes all-clear.
* **`publish-issue-failed`** replaces only this run's pending annotation with a short terminal
  notice and run link, preserving the report beneath it. It is status on an existing
  regression investigation, not a newly filed workflow-failure alert.

All issue commands except findings are a logged no-op when no rolling issue is open. Evidence
is still validated before lookup, so absence of an issue never hides invalid inputs. Commit
ordering and run ownership protect newer reports and pending annotations from delayed work;
an unverified ordering preserves existing findings with an explicit diagnostic.

**One-off failure alerts (history flow).** `alert` files an issue about a particular failed
workflow run, independently of the rolling regression issue. A later successful run neither
closes nor rewrites it: success does not explain the earlier failure. There is no automatic
alert-resolution command and no shared rolling failure issue.

The identity is repository, project namespace and **workflow run ID**, not run attempt.
Repeated calls and reruns reuse that run's alert; another failed workflow run gets another
issue. Title search includes closed issues, so a human-closed alert is not recreated or reopened
on retry. An existing alert is left unchanged. Creates use the same ambiguous-create
reconciliation as other publication, not blind retries. This is deduplication of one event,
not aggregation of unrelated failures.

Partial collection may therefore publish qualified findings and file an alert for the failed
job in the same workflow. It must not call `publish-issue-failed` over the successful report.
The companion owns these GitHub status messages; analysis vocabulary still comes from the
tool's rendered report. Alert publication requires an available companion. Bootstrap,
installation or companion-artifact failure remains visible as a failed workflow even when
that failure prevents the alert itself; no alternate publisher bypasses this dependency (§3).

### 4.5 Collect write mode and inferred analysis mode

**Collect write mode is an input, not a constant.** Because the action is trigger-agnostic
(§1), `collect` maps the `on-existing` input onto the tool's write-collision flags — `skip`
→ `--skip-existing`, `overwrite` → `--overwrite`, `error` → neither (**the default**, the
tool's own strict write-once behaviour). This is the concrete mechanism that lets a scheduled
collection and an on-push collection land on the same commit without a hard failure: such a
setup sets `on-existing: skip`, and `skip` additionally keeps the read cache (§4.2) valid
because a skipped write never arms its invalidation marker. Keeping the default at `error`
matches the CLI, so a caller who does nothing special gets the tool's own conservative
behaviour.

**`backfill` has its own supported write modes.** Densification (§4.8) is
defined by being resumable: it walks a window that it has usually already partly filled, so
erroring on an existing point would fail every run after the first. `on-existing` therefore
defaults to `skip` for `backfill` and to `error` for ordinary `collect`. For `backfill`, `skip`
passes no flag and `overwrite` passes `--overwrite`; `error` is rejected because the CLI has
no strict-duplicate backfill mode. It also has no `backfill --skip-existing` flag. The default
is resolved
**per command** rather than as one action-wide constant, so an omitted input means "this
command's sensible default", and an explicitly set one always wins.

**PR and trunk measurements share a store, not an analysis timeline.** Points are keyed by
commit, and Git topology determines which commits contribute to a query. PR commits absent
from the trunk's first-parent history do not enter trunk analysis, including commits
replaced by a squash or rebase merge. History collection and densification measure the actual
trunk commits. Both flows use ordinary immutable objects and the configured write-collision
policy; predefined CI workflows select `skip` so reruns retain existing measurements.
Stored PR points follow ordinary storage maintenance, without automatic PR pruning.

**Analysis mode is inferred by the tool, not selected by the action.** There is no `--mode`
flag: `analyze` auto-detects **history** vs **branch** from git topology and the recorded
runs it admits (`DESIGN.md` §8.5) — the analyzed tip being its own merge-base with no dirty
run means history; anything past the merge-base means branch. The action does not expose a
mode input; instead its two analyze *commands* pre-wire the inputs that put the tool in the
right mode: `analyze-history` sets `--context` and `--base` to the same collected commit and
excludes dirty snapshots (→ history), while `analyze-pr`
passes an explicit `--base` distinct from the PR-head context (→ branch). Future analysis
questions, if any, arrive as **new command values** with their own cohesive input group and
sink, never as an orthogonal mode parameter grafted onto one overloaded command.

**`examine` is a human follow-up, not a CI command.** The tool's `examine` drills into a
single `(benchmark, metric)` series and is what a maintainer runs by hand after a report
points at a finding. The action does not wrap it — it is interactive triage, outside the
collect/analyze/report loop.

### 4.6 Machine-key handoff between collect and analyze

Collection stamps **every** result with the runner's **real hardware fingerprint**, and
analysis must survey exactly those keys — but collection is a matrix across a heterogeneous
runner pool while analysis is one job that cannot re-derive those keys from its own hardware.
The action therefore treats the handoff as a first-class concern:

* `collect` exposes the leg's fingerprint as its `machine-key` output (§4.1). After success,
  the workflow writes a **collection receipt** binding that key to the repository, instance,
  run, attempt, frozen head and platform, and uploads the receipt as the per-platform
  **artifact**. Measurements remain in configured storage. A receipt confirms successful
  collection, including `skip` of existing objects, not freshly replaced measurements.
* Before analysis, the companion reconciles receipts with each platform's latest GitHub
  collection job attempt. A failed retry cannot reuse an older successful receipt; an
  untouched successful leg retains its earlier receipt. A successful job without matching
  evidence is an error. The reconciled set defines the **completed platforms**, compared
  with the matrix's **expected platforms** for publication coverage.
* Reconciliation writes the selected keys into the `machine-keys` directory consumed by
  `analyze-history` / `analyze-pr`. The composite action scans those selected files and
  passes each key as a repeated `--machine-key <fingerprint>` argument; it does not infer
  collection success from arbitrary downloaded files. Hand-assembled workflows perform
  the same receipt reconciliation before invoking analysis.
* **The download must be token-authenticated.** When a workflow is partially re-run, the
  artifacts it needs were produced by a *previous* attempt, and `actions/download-artifact`
  only resolves across attempts when it is given an explicit `github-token`. Without it a
  partial re-run cannot retrieve the receipt artifact and analysis cannot proceed. The reusable
  workflow (§4.7) passes `github-token: ${{ github.token }}`, `repository:
  ${{ github.repository }}` and `run-id: ${{ github.run_id }}`, with `actions: read`;
  a hand-assembled caller must do the same.

**Artifact authentication is available to fork-origin PR runs.** GitHub runs those
`pull_request` workflows in the base repository and supplies a read-only `GITHUB_TOKEN`.
That token can read the run's artifacts with `actions: read`; it is not an Azure OIDC token
or a caller-maintained secret. Repository policy may require approval before a fork workflow
starts. A workflow actually running in the fork has the fork's token, not unrestricted access
to another repository's artifacts.

The action still skips fork PRs (§6); artifact download is not the reason for that policy.
See GitHub's [fork workflow permissions](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#workflows-in-forked-repositories),
the [artifact action inputs](https://github.com/actions/download-artifact#inputs), and the
[run-artifact API](https://docs.github.com/en/rest/actions/artifacts#list-workflow-run-artifacts).

**Fingerprints are versioned, and a version bump partitions history.** The fingerprint is
derived from the usable hardware the runner actually exposes, and it carries an explicit
version tag. When the derivation changes, affected runners start
filing under a **new key**. Nothing is lost, but prior points remain under their key and no
longer join the new series, so a consumer sees a coverage gap until enough new points
accumulate (which is what the nightly densification pass, §4.8, exists to shorten).

**All** engines are machine-keyed — Callgrind (instruction counts) and `alloc_tracker`
(allocation bytes/counts) no less than the wall-clock engines (criterion and `all_the_time`) —
because even integer-count metrics turned out to be machine-dependent (libraries dispatch to
microarchitecture-specific code paths). There is no ride-along exemption: a result is only
analyzed when its own machine key was threaded in, so the handoff above is what makes *any*
engine's data visible to analysis, not just the wall-clock ones.

**The artifact steps stay in the workflow, not inside the action.** The upload and download are
plain `actions/upload-artifact` / `download-artifact` steps in the reusable workflows, which
already hide them from everyone on the default path. Bundling them into the composite action
would save two steps for hand-assembled callers only, and would cost us a permanent dependency
on two more actions to pin and upgrade, a fixed artifact-naming scheme to keep collision-free,
and ownership of their cross-attempt and retention failure modes. Since neither arrangement is
visible to most consumers, the tie breaks on which we would rather maintain — and the answer is
the one that adds nothing. Either way the tool stays GitHub-agnostic: it only ever sees
`--machine-key <fingerprint>`.

### 4.7 Two consumption layers — reusable workflows over composite actions

The commands above are *building blocks*. Assembling them into a working setup means writing
the same job graph every consumer needs: a matrix `collect` across platforms, the machine-key
artifact handoff, a single `analyze` gated on the matrix, the sink lifecycle jobs, plus
concurrency, permissions, and (for PRs) the same-repo gate. That graph is identical everywhere
except for its parameters, so making each consumer retype it is exactly the repo-specific
bulk this design set out to remove.

The action repo publishes these consumption layers:

* **Reusable workflows** (`workflow_call`) — the default path for history and PR reporting
  and historical densification: `history.yml`, `pr.yml` and `backfill.yml`. Each owns its entire
  job graph, and the consumer's whole workflow reduces to a trigger, a `uses:` line, the
  permissions the flow needs, and a few inputs:

  ```yaml
  on:
    push: { branches: [main] }
  jobs:
    bench-history:
      permissions:
        contents: read      # checkout
        actions: read       # cross-attempt artifact download (§4.6)
        id-token: write     # Entra OIDC, if using cloud storage
        issues: write       # rolling issue + failure alert
      uses: folo-rs/cargo-bench-history-action/.github/workflows/history.yml@v2
      with:
        platforms: ubuntu-latest, windows-latest
        azure-client-id: ${{ vars.AZURE_CLIENT_ID }}
        azure-tenant-id: ${{ vars.AZURE_TENANT_ID }}
  ```

* **Composite actions** — the escape hatch, and what the reusable workflows are built from.
  A consumer whose graph differs (extra gating, an unusual runner pool, a different sink)
  calls `collect` / `backfill` / `analyze-history` / `analyze-pr` / publication and lifecycle
  commands directly and wires the jobs themselves. Nothing is hidden from them; the reusable
  workflow is a convenience, not a privileged path.

**`backfill.yml` owns the densification job graph.** Callers supply required `azure-client-id`,
`azure-tenant-id`, and either an explicit inclusive `from`/`to` range or rolling
`lookback`/`minimum-age` durations with an optional `to` override. The caller chooses the
schedule and parameter values; shared Rust calculates and freezes the range. Folo's nightly
policy is one such configuration (§10), not a separate planning implementation.

Rolling durations use the same calendar-aware Jiff span conventions as `--since`, including
`24 hours`, `24 hours ago` and ISO spans. Their magnitudes are subtracted from one preparation-time
UTC clock snapshot. `lookback` must be nonzero; `minimum-age` may be zero. Both are required
for rolling selection, and neither accompanies an explicit `from`.
The automatic end is the newest eligible first-parent commit at least `minimum-age` old.
An explicit `to` bypasses that age selection. The start is the oldest first-parent commit
reachable from the selected end within `lookback` of preparation time, not of that end.
When the selected history is older than the horizon, the range collapses to the end alone.
No eligible automatic end is a successful, explained no-work result that starts no benchmark
jobs, distinct from fork-policy skipping and from empty current-head benchmark scope.

The workflow accepts the shared `platforms`, `working-directory`, `config`, `exclude`, `bench`,
`best-of`, `all-features`, `no-default-features`, `features`, `rustflags`, `install-method` and `source-path`
inputs. Collection covers the workspace at each historical commit, subject to exclusions.
There is no public package allowlist, scope/instance selector or setup-path input.
Optional `max-commits` is a string containing a positive integer within the executing
platform's `usize` range; empty or absent means unlimited replay. It reaches the core
as `--max-commits` without changing range preparation. The bound applies to replay attempts
after skipping recorded commits in the current partition, not to range endpoints.
`ignore-errors` is a Boolean input defaulting to false. It continues past per-commit
build/benchmark failures without suppressing infrastructure errors. The workflow preserves
unsuccessful job conclusions; there is no whole-job failure-suppression input. A hosted-runner
timeout is not a successful bounded pass.

Preparation uses full Git history to resolve selected nonblank, single-line refs safely to full
commit SHAs. It does not filter scope against current-head Cargo metadata or benchmark inventory:
historical commits can carry benchmarks absent from the invocation head. Execution checks out
the resolved `to` SHA with full history; configuration, optional source-built tools and the fixed
setup hook remain from the invocation checkout. The core tool owns first-parent range validation
and newest-first traversal, including the selected project directory within each worktree.

The shared matrix disables fail-fast, uses the hosted-job ceiling of 360 minutes, and fixes
`on-existing: skip` for resumability. It has no analysis, receipts, report artifacts,
publication sink or public outputs. Different write modes or a different job graph remain
available through the lower composite layer.

**`pr.yml` owns the scope preflight end to end.** Deciding *which packages a PR should
benchmark* is a prerequisite for the PR flow, and leaving it to the consumer would leave the
hardest part of adoption unsolved while claiming the flow is one `uses:` line. The workflow
therefore computes it as follows:

1. **Changed files → owning packages.** The diff against the base names files; each file
   belongs to a package. The companion uses `cargo-detect-package`'s read-only Rust library
   query for this lookup rather than installing another executable or duplicating its logic.
2. **Expand to dependents.** A change to a package can move the numbers of anything that
   depends on it, so the set is closed over reverse dependencies within the workspace.
3. **Keep the packages that carry benchmarks and apply configured exclusions.** `cargo
   metadata` lists every target with its kind, so benchmark detection is a filter over data
   Cargo already hands us — not a job for a package-detection tool. The workflow's `exclude`
   list then removes packages the repository chooses not to maintain in benchmark history.
   The same exclusions apply to history and backfill; no Folo package name is built into the
   generic action. Exclusions filter the final scope, not the reverse-dependency traversal:
   a changed excluded package can still affect a maintained dependent.

The result is the `packages` scope that drives collect (§4.1), and an empty result routes to
`publish-comment-inconclusive` instead of an analysis that would find nothing. It must never reach
`collect` as an empty list, because the lower-level command interprets that as the whole
workspace. The workflow owns this policy; a Rust helper performs its files-to-packages,
dependency-closure and metadata computations, rather than duplicating them in shell.

Placing this in the **workflow** layer rather than the action layer is the point. Repositories
genuinely disagree about what "affected" means — whether dev-dependencies count, whether a
workspace-wide config change touches everything — so this is exactly the kind of policy that
should be replaceable rather than baked into a published action's contract. A consumer who
disagrees with our answer drops to the composite layer, runs whatever preflight they prefer,
and passes the resulting `packages` to `collect`. They lose the one `uses:` line and gain full
control, which is the trade the two-layer split exists to offer.

These platform constraints shape the split, and none is worked around:

* **The Marketplace lists actions, not workflows.** Reusable workflows are referenced by
  repository path and ref (`owner/repo/.github/workflows/x.yml@v2`) and cannot be published
  to the Marketplace. The composite action therefore stays the Marketplace-listed artifact
  and the discovery surface (§8); the reusable workflows ride the same repo and the same
  `v2` floating tag, so both layers version in lockstep with one release. Inside the
  reusable workflows the root action is referenced through the **self-repository syntax**
  (`$/`), which resolves at the exact commit the workflow is running from — a hardcoded
  `@v2` there would let a workflow pinned to `v2.0.0` silently invoke a newer action.
* **`workflow_call` inputs are scalars.** Only `string`, `number`, and `boolean` exist, so
  list-shaped inputs (the platform matrix, package scopes) are passed as
  **comma-separated strings** and split inside the reusable workflow. JSON-in-a-string is the
  other common encoding and is rejected here: it is fiddly to write in YAML (quoting a JSON
  array inside a YAML scalar), easy to get subtly wrong, and produces an unhelpful failure when
  it is — a malformed array surfaces as a matrix expansion error rather than as "your input is
  wrong". A comma-separated list is what a human would write unprompted, and the same spelling
  works at both layers, so the composite action accepts it too rather than having one list
  syntax for the workflow and another for the action.
* **A caller matrix fans out the whole invocation, not one phase of it.** A matrix job *can*
  call a reusable workflow, but it re-instantiates the entire workflow per combination —
  which would give one analyze per platform instead of the single aggregate analyze the
  design requires. The collect matrix therefore lives **inside** `history.yml`, driven by the
  `platforms` input, so many collect jobs converge on one analyze. This is the main
  reason the reusable-workflow layer is worth having at all: the fan-out-then-converge shape
  is the single most awkward piece for a consumer to reproduce.
* **Secrets and permissions do not flow implicitly.** `secrets: inherit` only works when
  caller and called workflow share an organization or enterprise, so it cannot appear in a
  recipe aimed at any repository; the called workflow gets `github.token` regardless, and
  anything else is an explicit input. Permissions can only be **narrowed** by a called
  workflow, never widened, so the caller job must grant them — which is why they appear in
  the example above rather than being hidden. For the same reason, cloud credentials are
  passed as **inputs** (non-secret client/tenant identifiers, with the OIDC token minted
  inside), not by expecting the consumer to run a login step first: a caller job that
  delegates to a reusable workflow cannot contain steps at all.

**Custom benchmark setup uses one fixed local convention.** A calling job cannot add steps
around a reusable-workflow call, so a repository can supply:

```text
.github/actions/bench-history-setup/action.yml
```

Shared collection and backfill jobs invoke this action after checkout/bootstrap and before
benchmarking.
If the file is absent, the shared workflows run no custom setup.
The hook belongs to the repository/configuration checkout. It is not invoked by GitHub
publication jobs, and the shared workflow's own tool installation remains its responsibility.

There is no setup path/ref input, no hook-input map and no additional hook positions. The
repository owns any parameters within its local action. Folo reuses its normal setup through
a small wrapper:

```yaml
name: Benchmark setup
description: Prepare Folo benchmark prerequisites
runs:
  using: composite
  steps:
    - uses: ./.github/actions/setup-environment
      with:
        install-valgrind: "true"
```

Other repositories can put native dependency installation or code-generation steps in the
same conventional file. Static external `uses:` references inside it are ordinary GitHub
Actions composition; the benchmark action does not implement a dynamic action-reference
loader. Setup should prepare the environment without changing measured source identity.

The convention costs a small wrapper when an existing setup action lives elsewhere, in
exchange for one predictable location and execution point. Repositories needing a different
job graph or setup at several stages use the lower composite-action layer.

**`pr.yml` handles the PR's whole life, including its close.** A benchmark run takes hours, so
a PR closed or merged mid-run leaves one in flight producing a result nobody will read — a real
cost when each leg occupies a runner. GitHub cancels a superseded run when a *new* run joins the
same concurrency group, so reclaiming that time needs some workflow to start on the close event.
One possible shape is a second, tiny workflow that does nothing but join the group.

The action does not export that second file. `pr.yml` accepts the `closed` event itself and
skips every job when it fires: the workflow still starts, still joins the concurrency group, and
the in-flight run is still evicted — the mechanism is identical, but the consumer's surface
stays one `uses:` block instead of two files. The cost is one gate expression inside `pr.yml`,
paid once by us; the alternative charges every consumer an extra file whose purpose is
non-obvious and which is easy to omit, silently losing the cancellation. Trading our complexity
for theirs is the whole point of the layer.

**Collection queues are separate from supersession.** The predefined workflows queue PR
collection per platform across PRs, and push collection per platform across commits. Use
job-level `cancel-in-progress: false` with `queue: max`, and namespace the group by repository,
instance, flow and platform. Different platforms and analysis/publication jobs remain
independent. Workflow-level cancellation still replaces an older run of the same PR; history
deduplication targets the same commit, not different commits. Manual
history runs use their own groups rather than waiting behind the push backlog. Queueing
belongs to GitHub, not a running worker waiting on a lock.

Backfill does not deduplicate invocations by event or SHA and never cancels an earlier
invocation. Its run and work groups use distinct prefixes, separate from caller groups and
the reporting flows. Non-cancelling `queue: max` work queues are keyed by the canonical
storage project identity and platform, so different config paths naming the same project
cannot race each other.

**Merge queues do not add a benchmark flow.** Benchmark feedback is advisory and is not a
required branch-protection check. The PR workflow measures its frozen real head, not a
`merge_group` SHA, and does not subscribe to enqueue/dequeue activity. The history workflow
collects the actual branch tip after merge; backfill covers its ordinary history window.
Neither temporary queue refs nor their measurements need storage or Azure federation.

A queue can combine changes into a main push; per-push collection is not a promise to
measure every intermediate queued PR as a separate main commit. Squash/rebase still do not
promise that the original PR head survives. PR comments remain comparisons of the named PR
head/base, not a verdict on the queue's combined candidate.

GitHub requires **required checks** to run on `merge_group`. A repository must not make these
advisory benchmark jobs required without providing a queue-compatible check. This differs
from the action repository's required `install-tools` release gate, which does run for
merge candidates (§8.1). See [GitHub's merge-queue configuration](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/managing-a-merge-queue#configuring-continuous-integration-ci-workflows-for-merge-queues).

### 4.8 The densification flow (`backfill`)

Analysis needs *neighbouring* points, not just the commit under test: a lone measurement on
a fresh machine key has nothing to be judged against. Since each push measures only its own
commit — and a fingerprint version bump (§4.6) or a newly-added runner platform starts an
empty series — a series can stay too sparse to judge for a long time. A third flow closes
that gap.

* **`backfill` replays collection across a window of recent history**, walking **newest
  first** to prioritize the most comparison-relevant commits. Optional `max-commits` bounds
  attempts after skipping recorded commits, while completing each attempt's repetitions,
  engine storage and normal cleanup. Empty harvests, failed benchmarks and write-time
  duplicates consume attempts; pre-check skips do not. Bounded completion reports deferred
  work instead of claiming the entire range completed. It is **resumable by default**:
  commits already stored for this key are skipped, so later passes fill the next gaps.
  With explicit overwrite, a bounded pass starts at the newest commit on every invocation
  rather than advancing a cursor.
  The CLI takes inclusive `FROM TO` commit refs, not a duration flag. A reusable-workflow
  caller supplies either `from`/`to` or rolling duration parameters; shared preparation calculates
  the range and freezes it to full SHAs. `ignore-errors` maps to `--ignore-errors` when it
  should continue past a commit that cannot build or benchmark. It does not suppress
  infrastructure failures or unsuccessful job conclusions.
* **It has no analysis phase and no sink.** Densification only *writes*; the next
  push-triggered `analyze-history` picks up whatever landed. This keeps the flow free of
  report-sink concerns entirely — no issue, no comment, no staleness, receipts or reports.
* **Its natural trigger is `schedule`**, which is precisely the trigger-agnostic case §1
  calls out: a nightly densification pass and a per-push collector can target the same
  commit. The predefined backfill flow always skips existing data; custom composite callers
  may select overwrite when remeasurement is intentional (§4.5).

## 5. Configuration and the division of labour

This section settles related questions: what a consumer must *supply* to the action, and
where the action's behaviour actually *lives*. Configuration comes first, because it is the
smaller half — the interesting decision is the second one.

**Every public input needs a concrete purpose.** Inputs express repository configuration,
measurement choices or required workflow data. A core CLI option does not automatically
become an action or reusable-workflow option. Derive values from configuration and events
where possible, use fixed reporting policy, and use the lower action layer for genuinely
different job graphs rather than growing speculative controls in the standard workflows.

**Configuration is a committed file, not a scatter of inputs.** The tool already reads a
committed `.cargo/bench_history.toml` that describes *where*
history lives: `[project] id` (the partition key) and the `[storage.*]` backend. A generic
consumer is **assumed to commit that file**, exactly as the tool expects when run locally,
so the action does **not** synthesise a config from a scatter of per-field inputs. The unit
of configuration is a whole, reviewable, version-controlled file — not a dozen action
inputs.

* **Default** — no `config` input: the tool discovers `.cargo/bench_history.toml` in the
  checked-out repo and the action passes no `--config`.
* **Override** — the single **`config`** input (a path): passed verbatim as `--config
  <path>`. A caller who does not want the in-repo file, or who must assemble one from
  secrets at run time, writes their own config in a prior step and points the action at it.

**Runtime behaviour still comes from action inputs**, passed as CLI flags rather than baked
into the config file: `local-path` (→ `--local=<path>`), `cache` (→ `--cache=<dir>`, cloud
read-through cache, mutually exclusive with `--local`), the `collect` scope (`exclude` /
`packages` / `bench`), `best-of`, `on-existing` write mode, the
`machine-keys` handoff directory, the analysis context/base, and `since`. Collection,
backfill and import always derive the machine key from the real host; `--machine-key` is a
query filter, not a writing-side override.
The division is clean: the **config file says where history lives; the action inputs say what
this run does**.

**One repository may maintain several projects, so every derived identity is namespaced.** A
monorepo can reasonably keep more than one independent history — separate components, teams,
or hardware targets, each with its own `[project] id`. Nothing in the analysis prevents that,
but the *action* would collide with itself, because several of the identities it derives
default to a single constant:

| Derived identity | Collision if shared |
| --- | --- |
| PR comment marker | Two instances overwrite each other's comment on the same PR. |
| Regression-issue identity | Two projects overwrite each other's findings or all-clear state. |
| Failure-alert identity | One project's failure is mistaken for another project's already-reported run. |
| Collection-receipt artifact name | Matrix legs of different instances clobber each other. |
| History cache key | One project's history cache is served to the other. |
| Concurrency group | One instance cancels the other's in-flight run. |

These identities derive from the tool's **canonical storage project identity**, not the raw
configured spelling. After installation, Rust preparation obtains that core-resolved identity
and passes it internally to the workflow stages and companion. The core owns project
normalization; neither the bootstrap nor the companion duplicates it.

Configured spellings that resolve to the same storage project share an instance. This applies
to case variants and to IDs containing spaces, punctuation or non-ASCII text: storage and
reporting use the same equivalence rule. Where a GitHub field requires a safe representation,
its encoding represents that resolved identity rather than defining another normalization of
the original text. Distinct canonical storage projects remain independently namespaced.
There is no consumer instance or marker override.

The installed-binary cache is independent of project identity: it selects the tested tool
versions and runner platform (§3), not a project's history. It can be resolved before the
core binary supplies the project namespace.

Namespacing is part of the identity contract: the comment marker and stable issue-title
prefix are lookup keys. Keeping them tied to the project avoids cross-project interference
without requiring a second identity setting.

### 5.1 Where the logic lives — Rust binaries, not shell

Report composition and GitHub lifecycle policy belong in Rust. A shell formatter that
reinterprets every finding or unjudged-series reason duplicates the analysis model and can
drift when that model changes. Keeping the domain rendering in the tool and the GitHub
envelope in the companion avoids that duplication.

**After installation, substantive logic belongs in Rust.** The thin PowerShell bootstrap
(§3) obtains those binaries; it does not implement their semantic decisions. YAML holds job
orchestration and input/file wiring, not report interpretation or workflow-evidence policy.
Integration and deployment responsibilities are described in §12.

The companion's action execution boundary validates the composite's command-specific input
groups and invokes the main tool with argument vectors. It uses the core's configuration and
project-key helpers rather than another implementation of project normalization. Collection
and analysis remain in the main tool; the action boundary only coordinates those existing
operations and their validated outputs.

The split is drawn by **vocabulary ownership**, not by the more obvious-looking
"composition versus transport" line. These questions separate cleanly:

* *What do these numbers mean?* — findings and their direction, the coverage census and the
  reasons a series went unjudged, which commit a change point sits near. This vocabulary is
  the tool's, and it changes when the analysis changes.
* *What does a GitHub report look like?* — the heading, the hidden dedup marker, the
  analyzed-commit marker, the artifact link, the staleness banner, the in-progress
  placeholder, the failure alert. This vocabulary is GitHub's, and it changes when the
  reporting changes.

**Domain rendering stays in the tool.** The tool renders reports (`--markdown` for
the full report, `--markdown-summary` for a condensed one sized to fit a GitHub issue body,
and `--json`), including the **coverage verdict in prose** explaining *why* the judged set
fell short. The shared `cbh_render::Coverage` projection owns that vocabulary; the companion
embeds it rather than adding another mapping of unjudged reasons. The named `outcome` is
available in JSON and through `--outcome <path>`; the action derives its convenience
`notable` output as `outcome == findings`. No
report-publication logic enters the tool: it never learns what a comment marker or an artifact
URL is. Azure setup configures federated trust but makes no GitHub API calls.

**The GitHub envelope and the whole sink lifecycle live in the companion binary,
`cargo-bench-history-github`.** The name states what it is — the GitHub adapter for
`cargo-bench-history` — and follows the family convention the faker already sets
(`cargo-bench-history-faker`), so the relationship and the ownership are obvious on a
crates.io listing or a `PATH` entry. Naming it after the *action* was considered and rejected:
the action is a distribution wrapper that happens to invoke this binary, while the binary
itself is a GitHub adapter that would still make sense driven by a hand-written workflow, and
tying its name to the wrapper would misstate that. It wraps
the tool's rendered summary in the report body, adds the markers and the artifact link, and
owns *every* message — including the ones with no analysis behind them: the in-progress
placeholder, the staleness banner, the terminal failure notice, and the one-off failure
alert. It then performs the API work: finding the rolling comment by its marker, deciding
create-versus-edit, leaving the empty-scope note, filing the alert, and reading
the live PR head to detect a race.

**Ordering is why the envelope cannot live in `analyze`.** Values the body needs
do not exist when analysis runs. The report **artifact URL** only exists after the upload
step, which necessarily follows analysis; and the **live head distance** is deliberately read
as late as possible, immediately before posting, precisely so it catches a race that analysis
could not have seen (§4.3). A body composed during `analyze` would have to be rewritten
afterwards by whatever posts it — which is exactly the string-patching this design is trying
to eliminate. Composing in the companion, at post time, is the only ordering that works.

The companion stays **separate from** `cargo-bench-history` because the tool is a general
benchmark-history tool with zero GitHub API calls, and that neutrality is worth preserving: a
repo publishing to GitLab, or to nothing at all, should not install GitHub plumbing. Both
halves are still Rust and still unit-tested — the companion separates its **pure body
rendering** from a **fakeable GitHub client**, so every message is asserted without a network,
exactly as the sink bodies are pinned in §9's Layer 1.

Drift is prevented not by keeping the prose in one binary, but by making the companion
**embed** what the tool rendered rather than re-deriving it. The companion never maps a
finding kind or an unjudged-series reason itself; those arrive already rendered. If a new
reason appears in the tool, it flows through untouched.

**What is left for YAML — and what stays outside Rust entirely.** The action's steps invoke
the binaries, while these responsibilities remain outside them:

* **Installation bootstrap** — PowerShell selects the installation method and obtains the
  required released or source-built binaries before Rust can execute (§3).
* **Environment and credential wiring** — mapping federation variables into `$GITHUB_ENV`,
  creating a scratch directory. This is the runner's own contract; expressing it in Rust would
  mean shelling back out to set variables the shell must set anyway.
* **Artifact upload/download** — `actions/upload-artifact` and `download-artifact` are
  first-party actions with their own cross-attempt semantics (§4.6); reimplementing them would
  be strictly worse.
* **Job graph and gating** — `needs:`, `if:`, concurrency, and the same-repo check are workflow
  concepts. They are removed from the *consumer's* burden by the reusable workflows (§4.7),
  not by being rewritten in another language.

The division preserves these implementation rules:

* **Nothing parses human-readable output.** Report prose is for humans, not a parsing contract.
  Any value the action needs for a decision comes from the JSON report or a dedicated output,
  never from scraping text. In particular `--outcome` supplies the named verdict; `notable` is only
  a convenience for findings gating, never a substitute for the verdict when choosing a
  clean or inconclusive message or deciding whether all-clear is justified.
* **Retry is per operation, not per direction.** **Reads** retry with backoff behind a
  transient-fault classifier (a non-transient client or auth failure still surfaces at once);
  **updating a known issue or comment is idempotent** and retries too;
  only a **create** is genuinely ambiguous, because a failure
  may have taken effect. A create therefore does not blind-retry: issue discovery uses title
  search, comment discovery uses its marker, and reconciliation checks the intended content.
  A known issue number is read directly rather than rediscovered through the search index.

  A create can commit and then lose its response. The sink reconciles that ambiguous outcome
  against its intended identity and content rather than guessing that another create is safe.

**No labels.** The action applies none, and offers no input to configure any. Labels look like
free triage value, but they cost more than they return here: `gh issue create` rejects an
unknown label outright rather than warning, so a label named in configuration but absent from
the repository fails the whole filing — and it fails at exactly the wrong moment, since the
paths that file issues are the paths reporting that something is already wrong.
Making labels safe means creating them on demand, tolerating the permission to do so being
absent, and exposing per-repository configuration for names that every consumer will want
different — a chain of complexity in service of decoration. Everything works with zero labels
on the issues, so that is what ships. Consumers who want labels can add them by hand or by
their own automation.

**Rolling issues use server-side title search.** The standard title is:

```text
Benchmark history findings for <project> (updated YYYY-MM-DD)
```

The stable `Benchmark history findings for <project>` prefix is the lookup identity; the
date is not part of it. The companion queries GitHub's issue search rather than enumerating
every open issue and inspecting bodies:

```text
repo:owner/repository is:issue is:open in:title "Benchmark history findings for project-id"
```

GitHub supports quoted title phrases, not arbitrary substring or regular-expression matching.
The companion checks the returned title's exact project-qualified form locally, so similarly
named projects cannot match accidentally, then reads the candidate by issue number for its
current title, state and body. It paginates the narrow result set, not the repository's whole
issue list. Search errors, incomplete results, result-limit exhaustion and multiple exact
candidates are explicit failures, never evidence that a new issue should be created.
See [title search](https://docs.github.com/en/search-github/searching-on-github/searching-issues-and-pull-requests#search-by-the-title-body-or-comments)
and the [search API](https://docs.github.com/en/rest/search/search#search-issues-and-pull-requests).

The prefix is maintained by the companion, not a cosmetic override; it must be retained for
the issue to remain discoverable. No fallback scans all issues for renamed titles or adopts
older output formats. Body markers still carry report commit, run ownership and status, but
are not the repository-wide lookup mechanism. PR comments retain marker lookup within their
single PR, where issue-title search does not apply.

**The date means last report-body update, not last measurement.** Each body-changing operation
updates the title's UTC calendar date in the same issue update, including preflight, all-clear,
inconclusive and failed annotations. A no-op does not refresh the date. The body continues to name
the measured commit and any stale/incomplete state, so a recent title date cannot imply that
old findings were remeasured.

One-off alerts use the title `Benchmark history workflow failed for <project> (run <run-id>)`.
The same title-search mechanism omits the open-state filter for alerts, retaining closed-issue
deduplication without scanning historical issue bodies. Their run-specific titles do not roll.

**Search is not an atomic upsert.** Index visibility can lag writes; serialization of issue
writers does not make the search index read-after-write consistent. After an ambiguous create,
bounded reconciliation reads verify the intended identity and content. If they cannot establish
success, the operation reports failure and does not create again. Normal lookup uses the latest
available index; the design does not promise exactly-once creation in the face of index lag.
Detected duplicates require explicit resolution, not silently choosing one or deleting a thread.

### 5.2 Standard reports

Once the message catalogue lives in one binary (§5.1), standardisation is nearly free: there
is exactly
one implementation of each message, so every consuming repo posts recognisably the same
report. That is the default and it requires no configuration. A consumer who sets nothing
gets the full standard set — the in-progress placeholder, the results comment, the staleness
banner, the terminal failure notice, the clean and inconclusive messages, the coverage qualification, and the
failure-alert issue — all worded identically to every other consumer's.

**One rule binds the whole catalogue: a report never claims more coverage than it measured.**
Every message that reports a result states what it covered — the packages benchmarked, and the
completed platforms when they fall short of the expected platforms (§4.2). This is not a
per-sink courtesy but a property of the catalogue, because the states that most need it are
the quiet ones: "no regressions" after a platform silently dropped out, or after a series had
too little history to judge, reads as reassurance nobody computed. Since the messages are
selected by the named `outcome` (§4.2) and separately qualified by platform coverage, neither
an unjudged-series shortfall nor a missing platform becomes an unqualified all-clear.
Findings remain findings even when either coverage dimension is incomplete.

Titles, advisory wording, documentation links and identity markers are part of the standard
catalogue. The action has no cosmetic overrides, custom templates or legacy-output adoption.
The documentation link points to the tool's public book; the report's artifact link is
run-specific evidence, not a branding option.

Reusable workflows have one **`publish`** Boolean, defaulting to `true`. Setting it to `false`
suppresses all GitHub publication and lifecycle writes while still collecting, analyzing and
producing artifacts. This supports caller canaries without live writes; it is not a separate
custom-rendering system and does not promise reduced workflow permission declarations.
There is no independent regression-issue switch. Hand-assembled workflows can omit the
publication and lifecycle commands directly.

## 6. Storage backend & auth

Backend selection and credentials are resolved by the tool from the config file plus two
runtime signals; the action adds no GitHub-specific auth code and takes **no secret
inputs** — and, since the tool's Azure backend is **Entra-ID-only**, there is no secret
*storage* material for it to handle at all:

* **Local filesystem** — set `local-path`; the action passes `--local=<path>`, overriding
  any cloud backend in the config for that run.
* **Azure, Entra OIDC** — the config file's `[storage.azure]` names the account and
  container (non-secret, committable); it has no key or SAS field. Authentication self-mints
  via the tool's GitHub OIDC credential (`DESIGN.md` §6) when `AZURE_CLIENT_ID` /
  `AZURE_TENANT_ID` are present in the job env and the caller grants `id-token: write`. Those
  are **non-secret identifiers the caller sets in the job/step `env`** (or supplies a prior
  `azure/login`, which the tool's local-dev credential fallback then picks up): a composite
  action's steps inherit the job env, so the tool sees them without the action plumbing
  anything.

**Fork PRs are not supported.** The PR flow is **same-repo only**, enforced by an explicit
head-repository check before credentialed work or posting. The GitHub `pull_request` OIDC
subject does not distinguish same-repository and fork heads; neither that subject nor the
absence of stored secrets is the fork gate. Supporting fork contributions would require
additional execution and credential policy, which is outside this design's scope.

So the flow **detects a fork PR and stops early with a clear message** — that benchmarking is
skipped because the PR comes from a fork, which is a supported state rather than a
malfunction — and posts nothing. The message distinguishes an intentional skip from broken
or queued automation. Nothing else about the design is
fork-aware, and no input configures this.

**PR collection and analysis use the same store as the trunk.** Branch mode compares the PR
head against the trunk's recorded baseline, so both belong in the selected backend. PR
collection uses ordinary `collect --skip-existing`, and analysis reads the head and baseline
from that store. The matrix uploads collection receipts, not measurement objects. Analysis
selects the successful legs' machine keys from those receipts (§4.6).
An optional `--cache=<directory>` mirrors Azure reads; PR workflows restore but do not save
the Actions cache. Listings still come from Azure, so newly stored measurements remain
visible after restoring an older cache. `--local` selects filesystem storage instead of
Azure for both collection and queries; it remains an alternative backend. Integration
requirements are in §12.

**Provisioning is an explicit tool command, not an action side effect.**
[`cargo-bench-history setup-azure`](DESIGN.md#710-setup-azure) supplies the standard storage
and one federated managed identity without copying Folo's infrastructure files. Execution validates
Azure CLI, Bicep, PowerShell and the selected Azure login context, then runs the embedded
deployment bundle from a temporary directory. `--out-dir` exports the self-contained bundle
for review or modification without running anything or requiring cloud tooling. The book
documents the parameter and non-secret identifier handoff. Existing infrastructure remains
usable directly; the action never provisions resources during a benchmark workflow.

**Caller permissions** (documented in the README): `contents: read` (checkout);
`actions: read` (cross-attempt artifact download, §4.6); `id-token: write` (Entra OIDC
self-minting); `issues: write` (history publication and the
one-off `alert`); `pull-requests: write` (branch flow's comment and
its lifecycle). A job requests the permissions needed by its combined steps.

**One Azure identity; no mandatory analysis/publication job split.** The standard setup grants
one managed identity `Storage Blob Data Contributor` on the history account and federates the
selected history branch and PR subject. Collection, backfill and analysis use that same client
ID. The predefined history/PR workflows analyze, upload reports, and publish in one job.
Lifecycle jobs remain separate only where their timing requires it, such as preflight while
collection is running or terminal reporting after failure.

Azure and GitHub still authenticate to different services: the Azure identity is obtained
through OIDC, and GitHub operations use the job's built-in `GITHUB_TOKEN`. Neither credential
replaces the other, and neither requires a stored user token. Combining these capabilities
is intentional for the repository-owned workflows; same-repository code and actions running
with them are trusted with the granted access.

Fork exclusion remains explicit. Artifact-read token availability (§4.6) does not grant
GitHub posting rights or establish that running fork code with the production Azure identity
is supported. The design does not rely on a blanket claim that fork runs receive no token.

## 7. Interface summary (inputs / outputs)

This section describes the **composite action** — the building-block layer. The reusable
workflows (§4.7) accept a smaller, flow-shaped subset of the same names (plus `platforms`,
the comma-separated runner matrix) and pass the rest through, so a consumer on the default path
sees only the inputs their flow actually varies.

**Common inputs:** `command` (required; `collect`, `analyze-history`, `analyze-pr`, `backfill`,
the `publish-comment-*` / `publish-issue-*` states listed in §4, or `alert`);
`install-method` (`binstall` | `install` | `path`, default `binstall`; applies to
every binary the command needs, §3);
`source-path` (the Folo workspace root for `install-method: path`);
`working-directory` (the measured/configuration checkout, defaulting to the caller's working
directory); `config` (path to a
`bench_history.toml`; default: the tool's own `.cargo/bench_history.toml` discovery);
`local-path` (→ `--local=<path>`). The project namespace is resolved from configuration,
binary versions come from the release manifest, and verbose diagnostics are always enabled.
The working directory is independent of the installation source so Folo's separate checkouts
and isolated caller fixtures do not require copying tools into measured source.

**Cargo build inputs (`collect` / `backfill`):** `all-features` (default `true`, matching the
flows' need to reach benchmark targets gated behind `required-features`),
`no-default-features`, and `features` (a list). Without these a repo whose benches sit behind
a feature would silently measure nothing.

`rustflags` optionally appends rustc arguments using Cargo's whitespace-separated `RUSTFLAGS`
syntax, not shell quoting. The companion preserves effective ambient arguments, preferring
`CARGO_ENCODED_RUSTFLAGS` when present over `RUSTFLAGS`, and supplies the combined arguments
only to measurement child processes and the associated machine-key query. An empty input
leaves the environment unchanged. Rustc owns option interpretation; the action does not
invent a flag-normalization language. Callers choose any stability settings and use matching
inputs across history, PR and backfill.

**`collect` inputs:** `packages` (comma-separated list → `--package` per name; empty → whole
workspace); `exclude`, `bench` (→ repeated flags); `best-of` (→ `--best-of`, default 1);
`on-existing` (`error` (default here) | `skip` |
`overwrite` → neither / `--skip-existing` / `--overwrite`; §4.5). **Output:** `machine-key`
(this leg's fingerprint, for the analyze handoff).

**`analyze-history` inputs:** `machine-keys` (directory of collected per-platform keys →
repeated `--machine-key`); `cache` (→ `--cache=<path>`; mutually exclusive with `local-path`);
`context` (default `HEAD`; the resolved collected commit is passed as both `--context` and
`--base`); `since` (look-back window; default: the tool's history default).

**`analyze-pr` inputs:** `base` (→ `--base`; **no
built-in branch name** — the reusable workflow passes the PR event's own base ref, and the
composite layer falls back to the tool's configured default-branch resolution, so a repo whose
trunk is not `main` works without a branch-name assumption; a hand-assembled caller must
pass the PR's actual base explicitly when it differs from that default);
`context` (→ `--context`; default `HEAD`); `machine-keys`; `cache`. Improvements are reported
unconditionally in branch mode, so there is no direction input.

Both analysis commands also receive nonempty `expected-platforms` and `completed-platforms`
CSV values. Their coverage output uses this explicit evidence; a machine-key directory alone
cannot establish which collection jobs succeeded. The reusable workflow passes its validated
collection projection rather than asking the composite to rediscover the job matrix.

**Report-publication inputs:** `publish-<sink>-findings`, `publish-<sink>-clean` and the
report-bearing form of `publish-<sink>-inconclusive` receive the rendered summary, JSON report,
analyzed commit, artifact URL and expected/completed platforms. Comment commands additionally
take `pr-number` and `packages`. Titles and markers are standardized; these commands hold no
storage inputs. The companion's `--body-file` supplies the summary and `--report-file` the same
analysis pass's JSON metadata. The summary must contain non-whitespace text.
`--analyzed-sha` must match the report's commit, analyzed from an unmodified working tree;
this requirement is distinct from the `clean` analysis outcome.
`--expected-platforms` and `--completed-platforms` carry nonempty CSV matrix identifiers;
the latter lists only successful legs. No separate caller-supplied outcome overrides the JSON.

**Lifecycle inputs:** rolling publication carries `run-id` and `run-attempt` for ownership.
Preflight, failed and empty-scope commands use the frozen `head`; report commands use
`analyzed-sha`. Comment commands take `pr-number`; preflight also takes its nonempty package
scope. Failed commands take `run-url` and the terminal workflow conclusion (`failure` or
`cancelled`) so the notice does not call a cancellation a failed analysis.
The no-report form of `publish-<sink>-inconclusive` requires the explicit `empty-scope` preflight
result and rejects report evidence; omission of a report alone never means empty scope.
This is execution data, not configurable reporting policy. The predefined workflows derive
it from events, scope selection and validated artifacts.

`alert` takes the failed workflow's `run-id` and `run-url`, bound to the repository.
Attempts share the same run identity. It has no resolution counterpart or auto-close input.
The names and required evidence agree between the composite and companion layers.

**Composite `backfill` inputs:** the same scope inputs as `collect` (`packages`, `exclude`, `bench`,
`best-of`), inclusive `from` / `to` refs, `ignore-errors`, optional `max-commits`, and `on-existing` (`skip` by default
or `overwrite`; `error` is invalid here, §4.5). The reusable workflow accepts either explicit
`from`/`to` or rolling `lookback`/`minimum-age` with an optional `to` override. Shared preparation
calculates and freezes the range; callers supply parameters only. The workflow fixes skip-existing
workspace collection with configurable exclusions and preserves unsuccessful job conclusions (§4.7).
Both layers accept `max-commits` as an optional string with no default cap. The companion
validates its positive platform-sized integer value only for backfill and forwards it
unchanged to the core. Range preparation receives no attempt limit.

**History/PR reusable-workflow publication control:** `publish` (Boolean, default `true`) controls all
GitHub writes as one policy (§5.2). It is not an input to the individual composite commands:
analysis commands never post, while publication/lifecycle commands perform their named role.
There are no report-wording, marker, template or label inputs.

**Outputs (from `analyze-history` / `analyze-pr`):** `outcome` (`findings` | `clean` |
`insufficient_baseline` | `nothing_in_scope` | `partial` — the successful analysis verdict,
§4.2; a non-zero process result becomes `failed` in the workflow);
`notable` (`true`/`false`, retained as the convenience boolean for simple gating, and defined
as `outcome == findings`);
`partial-platform-coverage` (`true`/`false`, orthogonal to `outcome`);
`regressions` (count); `report-markdown` (full report path); `report-json`; `report-summary`
(condensed top-findings Markdown). Paths are local to the analysis job. History/PR reusable workflows
re-export verdict/count/coverage values and the report artifact identity/link, not paths
that a downstream job cannot access.

The tool's JSON report has **no schema-version field**, and the binaries expose no
report-protocol negotiation. Consumers independently processing those artifacts must pin and
test their tool/action selection; the action does not add a `report-schema` or templating API.

## 8. Versioning & Marketplace

* **Semver tags** `vX.Y.Z` on the action repo, plus a **floating major** `vX` ref
  that is force-moved to each new release of that major (the standard `actions/*` major-tag convention,
  re-pointed by the action repo's `release.yml`).
* **An action release selects an exact tested binary combination.** Tool and action version
  numbers are independent, but consumers do not override the manifest's binary versions.
  Each monorepo PR moving a pinned tool version has a paired action-release PR (§8.1).
* **README is a quick start; the book is the reference.** Two documents describing the same
  action drift, and the one that drifts is always the one a maintainer forgets — so they get
  clearly different jobs rather than overlapping scopes. The **README** answers "what is this
  and how do I switch it on": a sentence on what the action does, the history and PR caller
  snippets of §4.7, the permissions each needs, and a link onward for
  everything else. It stops there deliberately; a reader who needs more is a reader the book
  serves better. The **book's GitHub-automation section** (§11) is the reference: the input
  surface, the hand-assembled recipes for repos whose job graph differs, the deployment
  profiles, and how to read a report. It is also where the action's material sits next to the
  tool concepts it depends on — engines, comparability, analysis modes — which is the context a
  reader configuring a pipeline actually needs, and which a README cannot supply without
  restating the whole guide.

  Advanced compositions of the root commands follow these flow contracts:
  * A **per-push history** workflow — a `fail-fast: false` matrix `collect` job across the
    platforms (each `on-existing: skip`, uploading its successful collection receipt as a
    per-platform artifact), then an `analyze-history` job (`needs: collect`, `fetch-depth: 0`,
    downloading receipts **with an explicit `github-token`**, reconciling them against the
    latest collection job attempts to produce `machine-keys` and completed platforms (§4.6),
    and an `actions/cache` step feeding `cache`), then the workflow's
    report upload and issue publication selecting findings, clean or no-data in that same job.
    `publish-issue-preflight` and `publish-issue-failed` maintain existing issue status;
    `alert` files one-off workflow failures independently.
  * A **per-PR branch** workflow — a delta preflight computing the touched benchmarkable
    packages, a `publish-comment-preflight` job (in parallel with collect), a matrix `collect` job
    scoped by `packages`, an `analyze-pr` job (checkout `head.sha`, `fetch-depth: 0`,
    restore-only cache), then report upload and comment publication selecting
    findings, clean or no-data in that same job.
    Analysis and publication use `!cancelled()` so a superseded run never posts.
    The empty-scope `publish-comment-inconclusive` and terminal `publish-comment-failed` paths
    sit alongside them — all behind the same-repo check (§6).
  * A **densification** workflow (§4.8) — a matrix `backfill` job with the
    repository's selected platforms and historical window, with no analyze job and no sink.
  * The **concurrency** pattern: PR-driven runs
    cancel superseded runs keyed on the ref, and the close event is handled by `pr.yml` itself
    (§4.7) rather than a second workflow; the push flow deduplicates the same commit instead.
  Folo's history, PR and backfill callers use the shared orchestration; triggers and the
  backfill parameter values remain repository-owned.
* **Marketplace publish** from the action repo's release UI (root `action.yml` + branding)
  once a `vX.Y.Z` release exists. Only the composite action is listed; the reusable workflows
  ship in the same repo under the same tags but are referenced by path (§4.7).

### 8.1 Releasing the action (operator flow)

The action is distributed as git tags in its own repository. Its PR carries the manifest's
action version and tested tool pins; merging that PR to `main` triggers `release.yml`.
There is no manual tag-preparation step or crates.io publication in the action repository.

**Paired releases follow this order:**

1. A monorepo PR changes a pinned tool version, for any reason.
2. Its author creates or updates the linked action PR with the final pins and an appropriate
   action-version increment. The required `install-tools` check can fail while those versions
   are unpublished; the action PR remains blocked, not exempt.
3. The monorepo PR merges. Its asynchronous release workflow publishes packages, then the
   corresponding prebuilt archives. A merge or a successful registry-only publish is not
   evidence that the whole installation path is ready.
4. The author follows that publication and **reruns the action check** when its exact
   dependencies are available. Publication in another repository does not automatically rerun
   failed checks. Use ordinary authenticated maintainer/agent follow-up; no cross-repository
   dispatch service, polling workflow or stored credential is required.
5. Once the current action PR passes its required checks and review, it can merge.
6. The action's release workflow publishes the reviewed version and advances its floating
   major after the final availability gate.

The [repository-specific release policy](../../../docs/benchmark-action-releases.md)
and [`pair-benchmark-action-release` skill](../../../.github/skills/pair-benchmark-action-release/SKILL.md),
run after general `increment-versions`, require this pairing, including dependency/group and
version-only movements. The manifest is the scope authority, including its test-only faker pin.
Library dependencies reach the action through their pinned executable's release. Reuse an
existing pairing when appropriate and refresh both PRs after any version-plan change.
A tool-pin-only action change still carries an action-version increment.

**`install-tools` is a required, fail-closed availability check.** It reads the PR's own
manifest and invokes the shared installation canaries, not a second installer and not the
already-released action. For each manifest package and supported target, it:

* installs the exact registry version through `install` with the published lockfile;
* installs its promised prebuilt archive through `binstall` with source fallback disabled;
* verifies the selected executable version and exercises the command contracts used by
  the action and workflows.

The availability runs bypass the installed-binary cache and use isolated installation roots.
A version-range match, a source fallback, or `path` dogfooding cannot satisfy this check.
Normal consumer `binstall` retains source fallback (§3); only the release gate must distinguish
an absent archive from a working fast path. Missing packages/assets fail with their exact
version and target identified. No checksum manifest or alternate version-selection mechanism
is added. The gate also runs for the final merge candidate when a merge queue is used.

**The action's `release.yml` runs on pushes to `main` and supports rerunning failed releases.**
It reads the checked-in action version and pins from that immutable commit, reruns the shared
availability gate, then creates the immutable `vX.Y.Z` tag and GitHub Release and moves the
matching major tag. These operations are serialized and retry-safe: a publication retry's
existing version tag must name its original release commit, and an older run must not move
the major backwards.
Later commits with unchanged release-bearing content and manifest cause no new release and
leave the existing tag untouched. Changed action/workflow behavior or pins require a version
increment. A failure before the gate passes creates no release or tag movement; a partial
publication is reconciled on rerun.

The reusable workflows reference the root action at their own commit (§4.7), so a tag advances
both layers together. All tag/release operations occur within the same workflow; a tag created
with `GITHUB_TOKEN` is not assumed to trigger another workflow.

Maintainers review the chosen action-version increment and compatibility promise. For the first
Marketplace release they also complete the listing form (agreement, category and action metadata).
Subsequent action publication follows merge, independently of the monorepo's package publication.

## 9. Testing the action

Most of the action's risk is not in arithmetic — the tool owns that, covered by the monorepo
suite — but in behaviour that only manifests over *time* (a trend needs many commits before it
is "notable") and in **real GitHub side effects** (filing and updating a rolling issue; posting,
re-posting, stale-bannering, and cleaning up a PR comment). A single test level cannot reach all
of that, so the action repo layers three of them, each stronger and slower than the last.

**Layer 1 — isolated logic tests (every push, seconds, no network).** The PowerShell bootstrap
has direct tests with mocked installer output (§3). Substantive behavior lives in Rust (§5.1)
and is tested against fakes: machine-key gathering and the whole PR-comment
lifecycle — placeholder seeding, staleness-banner insertion/replacement, empty-scope notes —
asserted against a faked GitHub transport with **no live issue or PR**. This is also where
the exact composed issue/comment *bodies* are pinned (hidden markers, scope line, coverage
verdict, banner text), so a formatting regression fails here first — and because the
composition sits beside the data model it renders, a newly added census reason cannot slip
through unrendered.

**Fake lifecycle coverage is not HTTP-adapter coverage.** The companion also exercises its
REST adapter through injected HTTP and delay boundaries: serialized requests, pagination,
response decoding, retry/backoff, invalid responses and ambiguous-create reconciliation all
run without sockets or real-time waits. These tests cover the concrete adapter's policies,
not GitHub's live authorization or API behavior. Real-GitHub validation remains a distinct
layer below; neither fake-driven suite substitutes for it.

**Layer 2 — local-storage end-to-end on the CI matrix (every push, minutes, no secrets).** `test.yml` runs the *real* action against **local filesystem storage**
(`local-path` under `${RUNNER_TEMP}`) across the platform matrix and across *each* real
`install-method` (`binstall`, `install`, and `path`),
so both the install branching and the actual installs are exercised, not just mocked:

1. A tiny checked-in throwaway Rust project with one fast Criterion benchmark.
2. `command: collect` over `--local`; assert a result set was stored and the `machine-key`
   output is a valid fingerprint.
3. `command: analyze-history` over that store, threading the collected `machine-keys`; assert
   the named `outcome` and that the Markdown/JSON/summary reports exist and parse. A single
   measured commit is a collection smoke test, not evidence of a judged clean baseline.
4. A **branch fixture** — a throwaway repo whose final commit regresses the benchmark — drives
   `command: analyze-pr` (context = the tip, base = the branch point) and asserts
   `outcome == findings`, that the summary names the regressed series, and that the
   composed PR-comment body carries the scope line and hidden markers. The base side must be
   seeded with **enough points for branch mode to judge against** — a two-commit fixture cannot
   clear the detector's minimum evidence — so this fixture uses the faker→`import` path (§11)
   rather than real benchmark runs to populate the comparison window cheaply.
5. **Caller coverage for the reusable workflows.** Folo's synthetic caller invokes `history.yml`
   across the supported native platforms and verifies the report returned to the caller.
   Its real PR benchmark workflow invokes `pr.yml`. These calls exercise matrix expansion,
   fan-out-then-converge onto one analysis job, receipt and artifact transport, and the
   workflow's exported report outputs. Companion fake/native suites and action workflow
   contract tests cover partial and total collection failure, malformed platforms, empty scope,
   policy skips and run ownership. Contract tests check supplied inputs and output references
   against the actual action metadata; they do not require every advanced composite input to
   become a reusable-workflow input.
   The same synthetic caller invokes `backfill.yml` on Linux, Windows and Apple Silicon macOS
   over the frozen real event head and its first parent, using the nested faker fixture rather
   than wall-clock Criterion measurements. An isolated backfill project shares the existing
   test container. A separate caller verification job queries the core tool across all stored
   machines and targets. Its structured run listing must contain both current range endpoints
   as clean stored commits in one comparable partition per expected target; aggregate counts
   or older history cannot satisfy that check. It does not fabricate a verdict or require
   enough baseline to judge the history clean. This query and its failure-time listing are test evidence,
   not part of the public backfill workflow, which has no analysis, receipts or reports.
   Installed-tool smoke tests on the same runner separately prove skip-existing resumption;
   the hosted caller does not assume later invocations receive the same machine fingerprint.
   A Linux rolling call verifies automatic selection and single-commit fallback in a separate
   test project. A no-eligible call must show successful preparation with no executed
   benchmark jobs in current-attempt GitHub evidence. Exact cutoff, calendar and override
   behavior belongs to frozen-clock Rust tests rather than hosted-runner timing.

**Fork-gate validation exercises behavior.** Event fixtures cover same-repository and fork
heads, asserting the fork skip result (§6) and that credentialed and publication jobs cannot
run for that event. They exercise the gate and dependent-job conditions rather than asserting
copied workflow wording. When a genuine fork PR is available, it may also provide a live caller
canary for the skip path; a same-repository scratch PR is not counted as fork coverage.
This adds neither a maintained fork repository nor fork execution support.

The caller canaries run without publishing to GitHub
(`publish: false` for history/PR calls; backfill has no publication); composed-body checks use
the fake transport.
The hosted callers exercise Azure authorization; live posting uses its dedicated validation layer.

Publication cases assert the selected command and resulting state, not just `notable == false`.
They cover fully clean reports, findings with missing platforms, inconclusive reports, empty
scope and failed execution, including issue preservation when no all-clear is justified.
Same-run attempt races preserve newer placeholders and pending annotations; cross-run cases
exercise commit/live-head guards and serialized publication at the same commit.
Alert cases distinguish
same-run retries from different failed runs, preserve human-closed issues, and prove that a
successful run does not mutate prior alerts.

Issue discovery cases exercise server-side title query construction, exact project matching,
dated suffix changes, fresh reads by issue number, incomplete searches, duplicates and delayed
index visibility after a create. An injected clock proves UTC title dates change with body
updates, not with no-ops, without real-time waits. Unrelated repository issues must not cause
whole-repository enumeration. Workflow canaries retain the artifact handoff while proving there
is no report download or second Azure identity solely for publication.

The installation canaries also drive the required release gate (§8.1). Cases cover an
unpublished package, a registry version whose promised archive is absent, stale cached tools,
and an available exact tuple. A fresh check after publication must install the newly available
tuple without changing the manifest. Release orchestration covers gate failure before any tag
write, unchanged-version no-op, partial-release recovery, conflicting immutable tags and
out-of-order major-tag updates.

**Layer 3 — trend-dependent, real-GitHub-write validation.** The remaining gap is the behaviour
that needs (a) a benchmark history long enough to make analysis "notable" and (b) the action
actually writing to GitHub. The tool already ships the two enablers this needs — the hidden
`cargo-bench-history import` command and the published `cargo-bench-history-faker` engine
(§11). The same synthetic history supports real-repository checks and fast fake-transport
checks without a separate checked-in store or a maintained test credential.

* **Synthetic history against a real repository.**
  `cargo-bench-history-faker`
  writes curated per-engine output into a `target/`-shaped tree (inventing nothing — every value
  comes from its flags), and `cargo bench-history import --target-dir <tree>` stores that output
  through the exact `collect` finalize-and-store path *without running `cargo bench`*. Crucially,
  `import --commit <ancestor>` keys a stored point to any existing commit **without checking it
  out**, so a single HEAD position can fabricate a whole multi-commit series (a planted
  regression, a planted improvement) by looping faker→import over real ancestor SHAs. Both
  binaries are published and `binstall`-able, so the test needs **no vendoring and no
  workspace checkout** — it installs them like any consumer.

  **The repository under test is the action repo itself, not a separate sandbox**, and that
  choice is what keeps the credentials problem from existing. A workflow's built-in
  `GITHUB_TOKEN` is minted per run, expires with the job, is scoped to the repository it runs
  in, and needs no setup or rotation — so a job in the action repo that grants itself
  `issues: write` and `pull-requests: write` can exercise every write path the companion has:
  open a scratch issue, update it, resolve it to all-clear, open a throwaway PR, post the
  comment, re-post to prove in-place update, apply staleness and terminal notices, publish
  empty scope, and verify one-off alert deduplication without automatic closure —
  each asserted by reading the live repository back. A *separate* sandbox repository is what
  would force a long-lived credential, because cross-repository access needs either a personal
  access token or a GitHub App private key, both of which are exactly the maintained secret we
  refuse to introduce. Testing in-place removes the requirement rather than managing it.
  Because these runs mutate real issues and PRs (in a repo whose purpose is to host them), they
  are scheduled and pre-release rather than on every push, and their artefacts are namespaced
  by their configured test project ID (§5) so they cannot collide with anything real.
* **Compose-only assertions against a faked transport.** Reuse the same
  faker→`import` history in local storage, run `analyze-history` / `analyze-pr`, and assert the
  *exact* issue/comment body the action *would* post against a **faked GitHub transport** — no
  live posting.
  This catches body/marker/banner regressions with a realistic multi-commit trend without
  touching a real issue or PR, so it can run on every push. (It is
  Layer 1's body assertions, but fed
  a real long history instead of a one-off fixture.)

**Dogfooding is the fourth layer, and in practice the most valuable one.** Folo runs these
flows on every push and every PR against a real repository with a real multi-year history
(§10), which is precisely the condition the layers above simulate. It exercises the write
paths continuously, on data no fixture can imitate, and it does so with no extra credentials
because it is the repository's own `GITHUB_TOKEN` doing the writing. Its limitation is that it
only ever tests **one** configuration — the one Folo happens to use — so it can confirm the
common path works while saying nothing about the rest of the input surface. That is exactly
the gap the synthetic-history checks fill: they cover the configurations no production repo happens
to have, and dogfooding covers the realism no test fixture can.

Root-action installation canaries use local storage. Folo's cross-job caller canary uses
the separate Azure test account to exercise hosted authentication, history report handoff
and stored historical backfill data. Its independent backfill verification query does not
add an analysis phase to the reusable flow.
The monorepo's Azure-backend test jobs cover the backend's authentication branches (`DESIGN.md` §6).


## 10. Dogfooding — Folo's own workflows

Folo's deployed history and PR workflows consume a selected v1 revision of the shared action;
the nightly backfill caller pins a released v2 revision to its immutable commit. This section
describes those deployments; the general input contract above is v2.
**`install-method: path`** and **`source-path: .`** build the required tools from the invocation
checkout, so unreleased monorepo changes are exercised without waiting for tool publication.
The selected action revision supplies orchestration independently of those tool sources.
External repos use the default `binstall` install. Folo dogfoods the action's input-driven path
— config resolution, auth wiring, the `on-existing: skip` write mode and delta-scoped
`packages`, receipt-based machine-key selection, the `--cache` read-through cache, the analysis
outcome, and both report sinks with their lifecycles.

Folo's history, PR and backfill entry points are reusable-workflow calls rather than parallel
implementations of those job graphs. Domain rendering belongs to the tool, GitHub reporting
belongs to the companion, and configurable workflow policy belongs to the shared layer (§5.1).
Repository choices are inputs rather than duplicated shell logic.

The nightly densification caller keeps Folo's 02:00 UTC schedule, same-repository `main` gate
and repository-wide non-cancelling concurrency group. It passes `lookback: 14 days`,
`minimum-age: 24 hours` and an optional `to_commit` override as parameters. Shared preparation
owns the full-history queries, date arithmetic, frozen endpoints and successful no-work result.
`max-commits: '1'` bounds replay to one attempt per platform after skipping recorded commits,
without shortening the inclusive range or changing newest-first priority. This matches usual
nightly capacity, not a hard duration guarantee. Normal completion preserves full repetitions,
storage and cleanup and reports deferred work honestly; the six-hour ceiling remains an
exceptional watchdog.

Every production caller passes non-secret repository identity variables, exclusions, all
features, `best-of: 3` and matching compiler stability flags directly in `with`, with
`install-method: path` and `source-path: .`. The invocation-owned setup hook supplies genuine
build prerequisites only. There are no caller configuration jobs, shell calculations or
calculated outputs. The backfill caller leaves `ignore-errors` at its strict false default,
and the v2 workflow has no whole-job failure-suppression input. Build, benchmark and
infrastructure failures, including watchdog termination, remain visible as unsuccessful jobs.
The shared workflow owns the matrix with fail-fast disabled and skip-existing execution,
and leaves first-parent traversal to the core.

**The tested combination includes the action revision.** Building all binaries from one
checkout does not establish compatibility with an arbitrary action revision. Folo tests its
selected action/workflow revision with the selected source checkout. When action-facing commands
or arguments change incompatibly, the coordinated change updates Folo's caller reference to
the corresponding revision from the paired action PR. This uses the ordinary GitHub revision
reference, not another version-override input or protocol negotiation. Released installation
methods retain their exact manifest pins; source-pair validation does not replace the
[published-installation gate](#81-releasing-the-action-operator-flow), which follows monorepo
publication.

**What `install-method: path` requires, concretely.** The action executes inside the *caller's*
job, so the workspace it builds from is the caller's own checkout — nothing needs to be shared
between the repositories, and the action being hosted elsewhere costs nothing here.
These constraints are properties of the source input:

* **One `source-path` names the Folo workspace checkout.** The action knows the package
  layout of the source it owns; it does not ask callers for separate binary package paths.
* **All binaries come from that checkout.** A HEAD-built tool paired with a
  released companion is a version mix nobody tested; since `path` exists precisely to exercise
  unreleased code, it applies to every binary the command needs (§3).

The cost is a source build in the job, which for Folo is already paid — the workspace is being
compiled to benchmark it. The payoff is that a tool change is exercised by the same push that
lands it, rather than waiting for a release.

## 11. Tool and repository responsibilities

The tool, companion and workflow layer have separate responsibilities:

* **The tool owns the coverage verdict and named outcome.** `cbh_render::Coverage` already
  supplies the judged-set qualification to the human renderings and the structured census
  to JSON. `analyze` exposes the named verdict in JSON and via `--outcome <path>`, with
  `notable` retained in JSON as the findings convenience. No additional coverage formatter
  or standalone `--notable` flag is needed. GitHub report publication stays outside the tool.
* **The companion validates publication evidence.** Publication consumes `--report-file`
  from the tool's JSON output alongside the rendered `--body-file`, and comma-separated
  `--expected-platforms` / `--completed-platforms`. The report must name the requested commit
  analyzed from an unmodified working tree,
  the correct analysis mode, and consistent outcome/coverage facts. Missing platforms
  are disclosed in both sinks without hiding findings. `publish-issue-clean` requires the same
  evidence and refuses all-clear unless the outcome is clean and collection is complete.
  The command's explicit expected commit prevents silently using a report for another run.
* **Rolling lifecycle operations retain ownership.** Preflight records the frozen `--head`,
  workflow `--run-id` and `--run-attempt`; failed publication only retires its own pending state.
  Attempt ordering is local to a run ID; distinct runs use commit/live-head guards and follow
  serialized publication order when they name the same commit (§4.4).
  Empty-scope publication creates the
  explanatory empty-scope note even when there is no previous comment, and terminal notes
  become fresh placeholders when work resumes. Publication checks live-head freshness after
  comment lookup and preserves reports, placeholders and terminal notes owned by the current
  head. History publication and all-clear
  likewise preserve newer issue content when commit ordering cannot be established.
  Markers are derived from the project namespace, with no custom-marker or legacy-adoption path.
* **The companion is published and versioned independently.** New crates follow the
  first-publication process in [`RELEASING.md`](../../../RELEASING.md), with Trusted Publishing
  for subsequent releases. Version groups come from exact dependency edges, not an explicit metadata group:
  the tool and its `cbh_*` implementation packages move together, while the companion and
  faker are independent. Do not add a synthetic dependency to force lockstep. Subsequent
  released-content changes follow the ordinary version-increment process; the action
  manifest pins separately tested runtime and test tools (§3). A paired action PR adopts
  each pinned tool version movement; required installation checks gate its merge and release (§8.1).
* **CLI wiring must follow each command's actual contract.** `--config`, `--local=<path>`,
  `--cache=<path>`, `--best-of`, `--context` / `--base`, the query-only `--machine-key`,
  scope flags, and inclusive `backfill FROM TO` all exist. `collect` and `import` offer
  `--skip-existing` / `--overwrite`; `backfill` skips by default and only offers `--overwrite`.
  History analysis supplies the same context and base explicitly (§4.2).
  Direction follows the analysis mode (§4.3), without an action-supplied direction flag.
* **PR storage is the ordinary configured store.** PR collection persists measurements
  alongside the trunk baseline with `--skip-existing` (§6). Analysis reads that store with
  the frozen head/base and validated collection machine keys. The workflow layer supplies
  the shared identity, receipt handoff and restore-only Actions cache.
* **The tool supplies reusable Azure provisioning.** `setup-azure` executes or exports the
  package-owned, self-contained deployment bundle. The action never deploys infrastructure;
  the [command contract](DESIGN.md#710-setup-azure) and
  [bundle ownership](implementation.md#azure-provisioning-bundle) define prerequisites,
  safe deployment and the parameter handoff.
* **Workflow computation and publication have separate owners, not separate credentials.** The workflow
  owns package-scope policy and configurable exclusions (§4.7), using Rust for the
  computations and passing the final scope to the lower composite. Analysis emits reports;
  report upload and companion publication run in the same job (§6). The
  monorepo wiring does not itself deliver the external reusable workflows, and in-process
  lifecycle/HTTP tests are not live-GitHub validation (§9).
* **Synthetic-history tests use the tool's ordinary import path** (§9). The hidden
  `cargo bench-history import` command (`collect`'s finalize-and-store path minus the `cargo
  bench` run; `--target-dir` required, `--commit`/`--target-triple`/`--dirty` overrides —
  `DESIGN.md` §7.9) and the published-but-unsupported `cargo-bench-history-faker`
  engine together let a test job fabricate a realistic multi-commit history from published
  binaries alone.
* **No fake-engine handling needed** — the fake engine is its own separate package
  (`cargo-bench-history-faker`) with its own binary, so the published `cargo-bench-history`
  ships a single binary (`DESIGN.md` §9). The action installs the plain package name.
* **Monorepo helpers do not duplicate the shared implementation.** Collection, scope,
  artifact and reporting decisions belong to the shared Rust and workflow layers.
  Folo retains its triggers, window parameters and deployment-specific inputs in the thin caller
  described under [Dogfooding](#10-dogfooding--folos-own-workflows), while the source-built CLI
  owns measurement and storage. PowerShell handles bootstrap, repository
  workflow wiring and the independently executable Azure deployment bundle; its driver is
  shared by `setup-azure` and export, not reimplemented in Rust. PR-close cancellation belongs
  to the PR workflow itself.
* **The book has a "GitHub automation" section.** Running `cargo-bench-history` from
  automation is a primary deployment model, with questions that have no local analogue.
  The section covers:
  * **The flows** — per-push history, per-PR branch, nightly densification — and how they
    provide complementary feedback and history coverage.
  * **Adopting the action** — the full input surface and the hand-assembled recipes, with the
    action's README reduced to a quick start that links here (§8).
  * **Azure setup** — `setup-azure` execution, prerequisite checks and `--out-dir` export;
    non-secret configuration handoff and one shared production identity.
  * **Deployment profiles** — the shared, rotating, ephemeral runner pool versus dedicated
    self-hosted benchmark machines. These differ in almost every way that matters (machine-key
    stability, noise floor, whether densification is needed at all, useful `best-of` values),
    and a reader choosing hardware needs that comparison before they build a pipeline around
    one of them. This is **documentation, not a supported configuration we exercise**: our own
    runs are on shared public runners, and the dedicated-hardware profile is described so a
    reader can reason about the trade rather than because we test it.
  * **What automated measurements mean** — PR measurements remain stored under their branch
    commits. When squash or rebase excludes those commits from trunk ancestry, their points
    do not enter trunk analysis (§4.5); history and densification measure the actual trunk
    commits.
  * **Reading a report** — in particular telling apart "no findings", "not enough baseline
    yet", "some platforms did not report", and "nothing benchmarkable changed" (§4.2).

  The section is titled **GitHub automation** rather than "Continuous integration": the latter
  is a vague umbrella that says nothing about what the section contains, while this section is
  specifically about driving the tool from GitHub — the flows, the action, the reports it
  posts. A reader looking for it will be looking for GitHub, and a future reader adding, say,
  a self-hosted-runner chapter will know whether it belongs here.

  The division of labour: the **book** owns tool-level concepts, the deployment thinking, and
  the action's reference material; the action repo's **README** is a quick start that links
  there. Behavioral design and internal architecture belong to their owning package guides.

## 12. Integration and deployment

### Workflow execution

Source-built Folo automation uses a preparation job to build the companion from the
automation checkout and publish it as a run-scoped Linux executable archive. The combined
analysis/publication job and the independently scheduled lifecycle jobs use that executable. All
required Folo binaries use the selected installation method and source checkout (§3, §10).

Source-built PR automation uses the event's merge checkout while benchmarking and topology use a
separate full checkout of the frozen PR head. This makes updated automation available to PRs
whose head predates it without recording the synthetic merge commit as a measurement.
Collection passes that repository explicitly to the tool; analysis passes both it and the
event's frozen base commit. The reusable workflow owns collection-scope policy and passes
the selected scope to the lower action layer.

Successful collection writes a receipt containing repository, instance, run, attempt, frozen
head, platform and the actual machine key. The artifact contains only the receipt; measurements
remain in configured storage. Analysis reconciles receipts with each platform's latest GitHub
job attempt: a failed retry cannot reuse an older receipt, while an untouched successful leg
retains its earlier one. Missing evidence for a successful job is an error; total collection
failure produces no synthetic report. The selected successful machine keys scope ordinary
configured-store analysis.

The companion projects validated reports into workflow outputs selecting findings, clean or
no-data publication. Failed execution uses the separately owned terminal-status path.
History all-clear is permitted only by the complete clean-evidence projection. Publication
always receives the JSON, summary and exact platform set from that analysis, and reports
remain downloadable even when they contain no findings. Issue discovery uses project-qualified
title search; PR comments and report-state metadata use standard markers. Outputs using older
identity formats are not adopted, rewritten or removed.

Azure configuration supplies one production managed-identity client ID and tenant ID.
The identity supports collection and analysis in history-branch and same-repository PR
workflows; analysis may use it alongside the GitHub posting token in one job.
The preparation artifact is also required by notification jobs; inability to build or obtain
the companion remains a failed workflow check rather than a successful notification.
Bootstrap, installation and artifact failures do not invoke a separate fallback publisher.

**Validation follows the current repository recipes.** `just validate-local` performs shallow
validation. Miri, mutation testing, many-seed Miri and careful checking are separate deep
checks, invoked through their scoped recipes or `validate-deep-local`; they are not implied by
a shallow pass. The Standard validation workflow and the Deep validation workflow own their
respective scheduling. Updating the action does not require reproducing the monorepo's
scheduled-validation infrastructure.

### 12.1 Maintainer setup

Deployment and publication require maintainer actions independent of the workflow runs.
General provisioning uses [Azure setup](DESIGN.md#710-setup-azure); Folo-specific parameters
are documented in the
[production deployment guide](../../../infra/azure-bench-history-prod/README.md).

| # | Action | Gates | Notes |
| --- | --- | --- | --- |
| 1 | **Configure production storage and its identity** | Using the Azure-backed workflows | Use one federated managed identity for history and PR collection and analysis, record its non-secret identifiers, and verify storage access. |
| 2 | **Bootstrap new crates, then configure Trusted Publishing** | Installing published tool and companion versions | Follow `RELEASING.md`: first publication is a maintainer operation from clean `main` after review and merge; subsequent releases use the configured `folo-rs/folo` / `release.yml` Trusted Publisher. |
| 3 | **Configure Marketplace publishing** — agreement, category and listing | Public action release | A one-time UI flow tied to the account, not to a release run (§8.1). |
| 4 | **Define the `v1` compatibility promise** | Publishing and moving the floating major tag | Consumers inherit the release that the tag identifies; breaking changes require an appropriate new major. |
| 5 | **Require the action installation gate** (§12.2) | Merging and releasing the action | Protect `main` with PR review and a required check that cannot pass unless `install-tools` succeeds; asynchronous tool publication does not waive it. |

The design does not require:

* **New stored secrets or tokens.** Workflows use the per-run `GITHUB_TOKEN` (§9) and
  Azure federation. The production identity has a non-secret client ID and short-lived
  OIDC exchanges, not a maintained credential. The one-time crates.io bootstrap uses the maintainer's manual
  publication process; ongoing automation introduces no stored user credential.
* **A separate test repository.** Testing happens in the repository that runs it (§9), which
  is what removes the cross-repository credential problem entirely.
* **Issue labels.** The action applies none and configures none (§5.1), so no repository
  needs labels created before it can file.
* **A blanket write-token default.** Workflow/job `permissions:` blocks request their own
  minimum scopes. Repository policy still controls whether test workflows may create pull
  requests (§12.2).

### 12.2 Configuring the action repository

Runtime permissions, test-fixture permissions and repository governance are separate:

* **The action repository is public**, with action metadata at its root and its own
  release/tag stream (§2).
* **Workflow permissions are a default, not a ceiling.** A repository whose default token
  permission is read-only can still run workflows that request `issues: write`,
  `contents: write`, or `id-token: write` through a job's `permissions:` block — the monorepo
  does exactly this today. Leave the read-only default in place and grant writes only to the
  particular jobs that need them.

**Creating test pull requests has a separate repository setting.** If the real-GitHub tests
create scratch PRs using `GITHUB_TOKEN`, enable Settings → Actions → General → Workflow
permissions → **Allow GitHub Actions to create and approve pull requests** (subject to
organization policy). The fixture-creation job also needs `contents: write` to push a temporary
branch and `pull-requests: write` to open its PR. Commenting on an existing maintainer-created
fixture PR avoids this setting; issue/comment tests need only their corresponding write
permissions. This setting is a test-fixture prerequisite, not a prerequisite for implementing
or locally exercising the companion.

**The release-availability gate is required configuration, not optional hygiene.** Protect
`main` against direct/force pushes and require a reviewed PR. Require the action repository's
`required-checks` fan-in, which must require `install-tools` to succeed and reject skipped,
cancelled or failed installation legs. Run it for PRs without path-based omission, and for
merge candidates if a merge queue is enabled. This prevents merging a release manifest
whose selected tools are not yet installable (§8.1). The release workflow repeats the gate
before publication; it does not replace the merge gate.

Additional governance is independent of runtime behavior. The repository can use squash,
merge commits or a merge queue; its own commit history is not benchmark data. The release
workflow needs `contents: write` only in its tag/release job; availability checks need no
posting permission and no Azure identity.

One trap is worth recording, because it is the only setting that can silently break a release:
**if tag protection is ever added, it must exempt the release workflow.** Releases work by
pushing `vX.Y.Z` and force-moving `v1` (§8.1), so a protection rule added later for tidiness
would block the release rather than the mistake it was aimed at. Not adding tag protection is
a perfectly good answer; adding it without the exemption is the failure mode.
