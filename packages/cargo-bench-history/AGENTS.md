# Agent notes for cargo-bench-history

This is an async-by-default Tokio application. The architecture keeps all pure
logic (parsing, mapping, comparability, key derivation, message formatting)
synchronous and pushes async only to the IO edges, each modelled as a small
"port" trait with a real Tokio adapter and an in-`#[cfg(test)]` in-memory fake.

## Ports and fakes

The orchestrator (`commands::run::execute_run`) is generic over its ports and is
driven in tests by fakes, never by real IO:

* `process::BenchRunner` — real `TokioBenchRunner` runs the benchmark command's
  argv directly (no shell — `argv[0]` is the program and the rest are passed
  verbatim, so forwarded arguments survive spaces and quotes intact); fake
  `FakeRunner` records the argv and reports a canned exit status.
* `probe::EnvironmentProbe` — real `SystemProbe` (shells `git`/`rustc`, and
  reads the local hardware profile via `machine::system_profile`); fake
  `FakeProbe` returns canned `GitInfo`/`RustcInfo`/`HardwareProfile`.
* `bench_output::BenchOutputSource` — real `FsBenchOutputSource` walks the
  engine's output tree and returns a `Harvest` enum: `Harvest::Callgrind` from
  `target/gungraun/**/summary.json`, `Harvest::Criterion` from
  `target/criterion/**/new/{benchmark,estimates}.json` (only `new/` dirs holding
  both files), `Harvest::AllocTracker` from `target/alloc_tracker/*.json` and
  `Harvest::AllTheTime` from `target/all_the_time/*.json` (the two flat
  one-file-per-operation trees, scanned by `collect_flat`) — all `mtime`-scoped to
  the run. Fake `FakeOutput` is engine-aware and returns in-memory `RawSummary`,
  `RawCriterionCase` or `RawOperationFile` values.
* `storage::Storage` — real `LocalStorage` and `AzureBlobStorage`, selected at
  runtime by the `StorageFacade` enum that
  `build_storage` returns; fake `MemoryStorage`. `put` is **write-once** (an
  existing key yields `StorageError::AlreadyExists`); `put_overwrite` is the
  replacing escape hatch used only by `run --overwrite` (and, later, `backfill
  --overwrite`).
* `config_writer::ConfigWriter` — real `TokioConfigWriter` (creates parent dirs,
  `create_new` so an existing file is never clobbered); fake `MemoryConfigWriter`.
  Used by `commands::install::execute_install`. Its real adapter's IO error paths
  are covered by `#[cfg_attr(miri, ignore)]` real-filesystem tests in
  `config_writer::real_writer_tests` (blocked parent, interior-NUL open error).

When you add a new IO edge, follow the same pattern: a port trait with an
`impl Future` return (RPITIT, no `async_trait`), a real adapter, and a fake.

## CLI structure (clap derive + argument groups)

The CLI is built with **clap** (derive API). It was chosen over `argh` because the
command surface is wide and `argh` cannot group flags under named headings; clap's
`help_heading` lets each command's `--help` present functionally grouped sections.
The other workspace cargo tools (`cargo-detect-package`/`cargo-freeze-deps`) also use
clap, but their CLIs are trivial — do **not** assume they share this crate's grouped
argument conventions.

Flags are organised into functional groups, applied via `#[command(flatten)]` shared
arg structs so a group looks identical everywhere it appears (and so analyze/list/
prune stay in selection lockstep automatically — see those sections):

* **Environment & execution** — `--config`, `--repo`, `--verbose`.
* **Output** — `--format`.
* **Benchmark scope** (`run`/`backfill`) — `--workspace`, `--package`/`-p`, `--bench`.
* **Discriminant selection** — `--engine`, `--target-triple`, `--machine-key`. On
  query commands each is **repeatable** (a `Vec`), accepts the widening keyword
  `all`, and auto-detects the current machine when omitted (§4.3). On create commands
  (`run`/`backfill`) only `--machine-key` is accepted (single override); there is no
  `--engine`/`--target-triple` and no `all`.
* **Commit selection** — `--context` (the target ref, formerly `--branch`),
  `--base`, `--since`, `--until`. `--since`/`--until` filter by **commit** timestamp.
* **Data filtering** — `--no-dirty`.

`--dry-run` (a `prune` execution flag) lives in the **environment & execution**
group, not data filtering.

There are **no `--os`/`--architecture` facets** (operate on `--target-triple`
directly). **Subjects** are bare positional words, not flags: `prune` takes
`<commit>…`, `bless` and `analyze` take `<prefix>…` (benchmark-id prefixes, used
respectively to choose what to accept and to scope the analysis), and `list` takes
a required `runs|discriminants|blessings` selector (a bare `list` errors and names
the three).

## Verbose diagnostics (`report::Reporter`)

`--verbose` is accepted by every command (`run`/`backfill`/`analyze`/`install`)
and threads a `report::Reporter` through the relevant pipeline so an
otherwise-silent outcome can be diagnosed. The trait has `enabled()` (gate
expensive per-file note formatting) and `note(&str)`. Production uses
`StderrReporter::new(verbose)`, which writes `[bench-history] …` lines to
**standard error** only when verbose — never stdout, so machine-readable output
stays clean. Tests use the `#[cfg(test)]` `RecordingReporter` (records notes in a
`RefCell`, exposing `notes()`/`contains()`).
`bench_output::collect` takes `&dyn Reporter` and notes each directory scanned and
every file included/excluded/stale; `run` notes the argv, injected env, harvest
boundary, and each stored key. `analyze_with` notes the listing prefix, facet
filters, the resolved target/base/merge-base, the **auto-detected mode and the
reason for it** (which topology/data inputs it considered and that the on-disk
working tree is deliberately ignored), the resolved `--since` cutoff and where it
came from, and why each candidate object is included or excluded; `install` notes
whether it wrote or left the config. The reporter is `&dyn Reporter` (not `+ Sync`),
so the run futures stay `!Send` and Miri-driven via `block_on`.

Verbose notes must be **explanatory, not conclusion-only** — see
`docs/standalone-binaries.md`. State the inputs and the rule behind each decision
(e.g. the mode note names whether the tip is its own merge-base and whether a dirty
run is recorded) so the logic can be reconstructed from the log.

`analyze` also renders a non-verbose diagnostic *hint* (carried on `ReportInput`/
`JsonReport`, built by `empty_history_hint`) whenever facet-matching runs were
stored but none entered the analysis — most commonly when every run is a dirty
snapshot on a base-side commit (the "config file never committed" trap). The hint
makes a `0 runs` result self-explanatory without needing `--verbose`.

## Storage model (commit-centric)

`comparability::DiscriminantSet` builds object keys under the partition prefix
`v1/<project>/<engine>/<triple>/<machine|synthetic>` and then keys each point by
**commit**:

* `clean_key(commit)` → `…/<commit>/clean.json` — the one canonical point for a
  clean working tree at that commit. It is deterministic, so a re-run of the same
  commit maps to the same key; the write-once `put` turns that into a
  `RunError::Duplicate` (refused) unless `--overwrite` switches to `put_overwrite`.
* `dirty_key(commit, observation_unix)` → `…/<commit>/dirty-<observation_unix>.json`
  — a snapshot of an uncommitted tree, distinguished by its observation second so
  multiple dirty snapshots on one base commit coexist; only a same-second clash is
  a conflict.

The commit segment is the **full** SHA (`git.commit`, `unknown` when there is
no repo), because the git-aware `analyze` reads `v1/.../<full_sha>/` directories
resolved from `git rev-list`. The `run` store step picks clean vs dirty from
`git.dirty`. A run stores only an **observation** timestamp (wall-clock now,
provenance only); the commit's position on the timeline is its committer date,
read from git topology at analyze time, not stored on the object. There is no
"effective timestamp" concept and no `--timestamp` override. A dirty key uses the
observation second so concurrent snapshots coexist. There is no run-id in the key.

### Object byte format (gzip)

Object **keys/filenames are unchanged** (still `…/clean.json` etc.), but the
**body bytes are gzip**, not plaintext JSON. The storage layer compresses
transparently at the `Storage` trait boundary: `put`/`put_overwrite` compress the
caller's plaintext JSON and `get` inflates back to plaintext, so every caller
(`run`, `backfill`, `analyze`, `prune`, `bless`) keeps handing and receiving plain
JSON and is unaffected. The single source of the encoding is
`cargo_bench_history_core::codec` (`compress`/`decompress`, gzip level 6, pure-Rust
`miniz_oxide`, deterministic, Miri-clean); both the storage backends and the stress
harness call it, so they cannot drift apart. `AzureBlobStorage` additionally sets
`Content-Encoding: gzip` on upload so a non-SDK reader can inflate the blob, though
the backend still inflates on `get` itself rather than relying on the service.
`MemoryStorage` (the test fake) deliberately stays **plaintext** — its contract is
the key/value model, no test inspects raw stored bytes, and keeping it uncompressed
keeps the Miri-driven `analyze` suite fast. This is a breaking storage-format change
(there is no migration): `get` on a legacy plaintext object errors loudly because
the gzip magic never collides with JSON's first byte.

## The `analyze` command

`analyze::execute` builds the real `SystemGitHistory` (rooted at `--repo` or the
current directory) and the storage from `build_storage`, then delegates to
`analyze::analyze_with`, which is generic over both the `GitHistory` and `Storage`
ports so tests drive it with `FakeGitHistory` + `MemoryStorage` +
`futures::executor::block_on` (Miri-safe, no Tokio). Everything below the IO ports
is pure and synchronous.

`analyze` assembles a series by **resolving git topology at query time** rather
than reading a flat storage prefix — the storage layout cannot pre-assemble a
timeline because which commits belong to a line of history depends on the branch
being analyzed:

* **Discriminant facets first.** `analyze::discriminant::parse_key` turns each
  `v1/<project>/<engine>/<triple>/<machine|synthetic>/<commit>/<file>` key into a
  `DiscriminantSet` (engine, triple, machine). `--engine`/`--target-triple`/
  `--machine-key` select sets — each facet is **repeatable**, accepts the widening
  keyword `all`, and auto-detects the current machine when omitted (`--target-triple`
  → host triple, `--machine-key` → host fingerprint, `--engine` → all engines since
  it has no machine-derived value). A `synthetic` set (Callgrind / `alloc_tracker`)
  **always passes** the `--machine-key` facet regardless of its value, so the
  current-machine default still includes every hardware-independent set. Each
  surviving set becomes its own sub-report. (There are no `--os`/`--architecture`
  facets — operate on the triple directly.) The `list discriminants` subject (not
  `analyze`) prints the present sets and returns **without requiring a repository**
  (it is a pure index over storage keys).
* **Repository required for analysis.** Resolving a timeline needs git, so when not
  just listing discriminants `analyze` errors if no repository resolves. The target
  ref is `--context` (default `HEAD`); the base ref is `--base` >
  `config.project.default_branch` > detected default branch (`origin/HEAD` → `main`
  → `master`). `analyze::selection::select_commits` walks the target's
  first-parent ancestry and splits it at the merge-base with the base: commits on
  the base side admit **clean runs only**; commits unique to the target side also
  admit **dirty** snapshots (so the official line stays clean while a feature branch
  can carry work-in-progress points). `--no-dirty` drops dirty everywhere. **One
  exception:** when the working tree is currently dirty (`GitHistory::is_dirty`) and
  the target **tip** is base-side, that tip's dirty snapshots are admitted and the
  report ends with an ephemeral-data warning — the "evaluating the tool" /
  "accidentally on the base branch" case. It is tip-only and `--no-dirty` overrides
  it. `select_commits`'s fourth parameter (`dirty_tip_exception`) carries this, and
  `SelectedCommit::dirty_base_exception` flags the tip so `analyze` knows to warn.
* `analyze::series` reconstructs one series per `(location, benchmark, metric)`
  ordered by **git topology** (first-parent index of the commit), then within a
  commit by `(dirty, object key)` so a clean point precedes same-commit
  dirty snapshots deterministically. Topology — not any timestamp — is primary, so
  back-dated backfill runs still sort by where their commit sits in history.
  `--since`/`--until` drop whole runs outside the window by each commit's **committer
  date**. `analyze`/`list` decide this from git topology — each commit's committer
  time is carried alongside its SHA on the `first_parent` ancestry
  (`FirstParentCommit`), so the window is applied in `select_dataset`'s key-only
  phase, *before* any out-of-window object is fetched (the `window_excludes` helper;
  a commit with an unknown time is kept). `prune` decides the same window from the
  same git-topology `commit -> committer_time` map (no per-object fetch). Bare
  `<prefix>` positionals scope the analysis to
  benchmarks whose qualified id starts with one of the prefixes (`series::prefixes_accept`).
* `analyze::findings` is the **engine-aware, noise-resistant** detector. It splits
  metrics into *deterministic* (every Callgrind kind plus the `alloc_tracker`
  allocation kinds — exact, no noise) and *noisy* (`WallTime` and `ProcessorTime`;
  `is_deterministic` decides). It emits at most
  one finding per series, of one of two methods: a **change-point** (sustained level
  shift) located by the **Pettitt** test, or a **drift** (slow monotonic trend) from
  the **Mann–Kendall** / **Theil–Sen** pair. When both fire, the better-fitting model
  wins (step vs line residual). Both regimes of a change-point need `min_regime`
  points (persistence — a single blip never flags). A deterministic step flags on
  persistence alone (any non-zero step is real). A noisy change additionally requires
  a significant **Mann–Whitney** rank test, non-overlapping regime CIs (when present —
  both Criterion and `all_the_time` report a bootstrap CI, so the gate applies to
  both; only older mean-only output skips it),
  and a practical-magnitude floor; a noisy drift also clears a noise floor of twice
  the median CI half-width. Noisy candidates pass a **Benjamini–Hochberg** FDR filter;
  deterministic ones bypass it. **Pettitt only locates the split — its analytic
  p-value is too conservative on short series, so it is never used as a significance
  gate.** All math lives in pure, Miri-safe `analyze::stats`; keep it deterministic
  and cover boundaries with named value-asserting tests, not threshold guards. When
  seeding test histories, a noisy (Criterion / `all_the_time`) step needs **≥ 4
  points on each side** for the rank test to have power, while a deterministic
  (Callgrind / `alloc_tracker`) step needs only
  `min_regime` (2) — and a single elevated last point can no longer flag either.
* `analyze::report` renders text/json/markdown. The top-level aggregate carries a
  `sets` array (one entry per discriminant set). Rendering is infallible: the
  report is plain structs of finite numbers, so the JSON path uses `.expect`
  rather than threading a serialization error nobody can trigger. Findings carry no
  severity tier — they rank by descending `|relative_delta|`. The `text` report is
  one paragraph per finding (headline leads with the relative-change percent; dimmed
  detail line; in **history mode only** a colored `rasciigraph` line chart of the
  series). `analyze_with` receives an explicit `color: bool` (production computes
  `stdout().is_terminal() && NO_COLOR unset`; tests pass `false`) and `render_text`
  calls `colored::control::set_override(color)` so both `colored` styling and the
  chart honor it deterministically (and stay Miri-safe — no isatty syscall). Text
  and Markdown values are rounded to four significant figures via `format_value`
  (the integer part is never truncated); the JSON keeps full `f64` precision.

The read-only `git_history::GitHistory` port (`resolve`, `default_branch`,
`merge_base`, `first_parent`) has a `SystemGitHistory` adapter that shells
`git -C <repo>` and a `#[cfg(test)]` `FakeGitHistory` over a canned commit graph.
`first_parent` returns `FirstParentCommit { sha, committer_time }` per commit
(`git log --first-parent --reverse --format=%H %cI`), pairing each SHA with its
committer date so the `--since`/`--until` window is decidable from topology alone.
Integration tests build a **real** git repo in a tempdir (`Workspace::repo` /
`Workspace::clean_repo`) and seed storage keys under the commits' real SHAs so the
live topology and the stored keys agree; `Workspace::commit_dated` pins each seed
commit's committer date so the git-topology `--since`/`--until` window matches.

### Analysis modes (`analyze` only)

`analyze` runs in one of three modes, resolved in `analyze/mod.rs` (`auto_mode`,
`parse_mode`, `resolve_since`) and dispatched in `analyze::findings::find_changes`
on `AnalysisMode`:

* **history** — auto-selected when the analyzed tip *is* the merge-base with the
  base **and** no dirty run is recorded on top of that tip (the official base-branch
  view: `auto_mode(tip_is_merge_base=true, dirty_tip_run_present=false)`). Applies
  the long-range change-point + drift + FDR techniques above. `resolve_since` gives
  it a **default six-month look-back** when `--since` is omitted
  (`default_history_since`, calendar-correct months via jiff); branch/tip have no
  default. `AnalysisContext::keeps` reports **regressions only** unless
  `--include-improvements` (steady improvement on the base branch is expected).
* **branch** — auto-selected otherwise: there are commits past the merge-base (a
  feature branch), or a dirty run is recorded on the base tip and admitted by the
  dirty-tree exception. `evaluate_branch` splits the series at the merge-base
  (`split_at_merge_base`), then `latest_regime` uses Pettitt to find the most recent
  within-branch flip and compares **only the after-segment** against the base,
  setting `Finding::flipped_at` to the commit the latest regime began at — so "got
  better then worse" reports worse and points at the flip. Reports **both**
  directions.
* **tip** — never auto-selected; forced with `--mode tip`. `evaluate_tip` compares
  the last point against a bounded window of recent preceding points. Reports
  **regressions only**.

Auto-detection is **data-driven, not working-tree-driven.** `dirty_tip_run_present`
(the second `auto_mode` arg) is computed in `select_dataset` from the recorded
candidates (a dirty run exists on a commit whose `dirty_base_exception` is set) —
**not** from `git.is_dirty()`. So a dirty working tree that has only ever stored
clean runs stays **history** mode; only an actually-recorded dirty run on the base
tip (or commits past the merge-base) flips it to branch. The on-disk tree state is
deliberately never consulted for mode selection (it still gates *admission* of
dirty runs in the §dirty-tree exception, a separate concern).

`--mode auto|history|branch|tip` overrides detection (`parse_mode`; `auto`/absent →
`None` → auto-detect; unknown → `RunError::Analyze`). **Findings never affect the
exit code** — `RunOutcome::Analyzed` is always a success; the machine-readable
signal is the `json` report's `mode` / `notable` (any finding survived) /
per-finding `direction` / `flipped_at` / `series`. There is **no**
`--fail-on-regression` flag: findings are advisory, never a build gate.
The two driving scenarios are a scheduled base-branch regression watch (history) and
a per-PR feature-branch evaluation (branch). Modes apply to `analyze` only; `list`
and `prune` reuse the same data-set *selection* but never analyze, so `--mode` /
`--include-improvements` / `--include-inactive` are analyze-only and **not** part of
the selection lockstep.

### Blessings and re-baselining (`analyze` history mode only)

A **blessing** manually accepts an intentional performance change on the base branch
so history analysis re-baselines past it and stops re-flagging it. Blessings matter
**only in history mode** — branch/tip modes treat the base as fully blessed already
(they compare against the base, not within it), so they ignore blessings entirely.

* **Data model — append-only sidecars.** A blessing is a
  `…/<machine|synthetic>/<commit>/bless-<issued_unix>.json` object alongside the
  commit's `clean.json`. It records the blessed benchmark-id prefixes and the
  issuing commit (there is **no** `reason` field — it was removed to keep the model
  small). Sidecars are append-only and never mutate a `clean.json`; `unbless`
  deletes the sidecar(s) at the context commit, and `run --overwrite` drops a
  commit's stale sidecars when it rewrites that commit.
* **`bless` rules (hard errors, no `--force`).** `analyze::bless::bless_with`
  requires: the context commit (HEAD by default, or `--context <ref>`) is on the
  base branch (`merge_base(context, base) == context`) and a `clean.json` already
  exists at that commit. The on-base check fires **first**. Neither state would
  survive a history analysis, so a feature-branch / data-less blessing is rejected
  rather than silently ignored. A **dirty working tree is allowed** (the blessing
  targets the committed `clean.json`) but emits a `Warning: uncommitted changes
  present …`. `bless <prefix> [<prefix> …]` matches each prefix against the qualified
  `<package>/<group>/<case>/<value>` identity via `starts_with` (per-benchmark, not
  all-or-nothing); `--all` (mutually exclusive with `<prefix>`) accepts **every**
  benchmark recorded at the context commit. It accepts the same discriminant facets
  as `analyze`.
* **Effect on analysis — active/inactive segments.** In history mode
  `select_dataset` loads `dataset.blessings` (only for in-window base commits). A
  blessed benchmark's *active window* starts at `max(last_blessed_commit,
  first_active_segment_commit)`; the change-point/drift detectors only consider the
  active segment, so a step that predates the blessing is no longer reported. Earlier
  (inactive) points stay in the series for charting (greyed) and for any long-range
  technique that needs more data. A `Finding` carries `active`, `blessed_at`, and
  `blessed_commit_time` so a consumer can see what re-baselined it.
* **Resolved spikes.** Independently of blessings, a spike that has since recovered
  to its prior level is **inactive by default** — its current state already matches
  the baseline, so re-flagging it is noise. `--include-inactive` surfaces such
  findings (`active: false`, `flipped_at` = the recovery commit) for auditing. The
  JSON `regressions` count and `notable` flag include any inactive finding that is
  present, but inactive findings are absent by default.
* **`unbless`** (`unbless_with`) deletes the blessings recorded **at the context
  commit** (HEAD by default, or `--context <ref>`) only — it never touches blessings
  defined at later commits, so the timeline can remain blessed past the unblessed
  commit. It reports `"Removed <count> blessing[s] at commit <short>."` (via
  `count_noun`, which computes the singular/plural) or `"No blessings recorded …"`.
* **`list blessings [--all]`** audits blessings without analyzing: the default
  (`scope: "head"`) lists blessings recorded at HEAD; `--all` (`scope: "window"`)
  lists the most recent blessing of every benchmark across the analysis window. See
  `analyze::list::blessings_at_head` / `blessings_across_window`.

## The `list` command

**`list` mirrors `analyze`'s data-set-selection parameters exactly** (a hard
requirement — keep them in lockstep). It takes a required **subject** — `runs`,
`discriminants`, or `blessings` — and a bare `list` is an error that names the
three. `list runs` and `list blessings` accept the same selection flags
(`--repo`/`--context`/`--base`/`--engine`/`--target-triple`/`--machine-key`/
`--no-dirty`/`--since`/`--until`/`--format`/`--config`) but, instead of
analyzing, only *preview* which data set an `analyze` pass would consume: `list
runs` reports, per discriminant set, the run, series, and per-commit counts of the
selected runs (each commit's clean/dirty split), ordered oldest-first by git
topology. Whenever you add or change a selection parameter on `analyze`, add the
same parameter to **both** `list` and `prune` (see below) unless it is genuinely
inapplicable. The analysis-only flags `--mode` / `--include-improvements` /
`--include-inactive` are not part of the selection lockstep (they govern analysis,
which `list` never does).

`list discriminants` lists the discriminant sets present in storage **without
requiring a repository** (it ignores the timeline / data-filtering groups). It is a
**discovery catalog**, so — unlike every other query command — it does **not**
default omitted facets to the current machine; with no facets it lists *every*
stored partition (`resolve_facets(&selection, None)`), and a facet narrows it.
`list blessings [--all]` audits blessings instead of previewing the data set (it
requires a repository) — see the Blessings subsection above. `--all` is a
`blessings`-only switch: `list_with` errors (`RunError::Analyze`) if it is passed
with `runs` or `discriminants` rather than silently ignoring it.

`list` lives **inside** the analyze module tree as `src/analyze/list.rs`
(`pub(crate) mod list;`), reusing the selection pipeline that was extracted from
`analyze_with` into `analyze/mod.rs`: `Selection` (a borrow of the selection
fields, built via `from_analyze`/`from_list`), `parsed_facets`,
`facet_filtered_candidates` (shared by both the discriminants index and the
topology query), `select_dataset` (git topology + commit selection + object load),
and `dirty_base_exception_warning`. `list_with` parses the format and subject, builds a
`Selection`, then either renders the discriminants index (no repo) or runs
`select_dataset` → `build_listing` → `render_listing`, returning a
`RunOutcome::Completed { message }`. `count_noun` (text.rs) appends `s` when the
count ≠ 1, so list.rs uses a local `series_noun` to avoid "seriess".

## The `prune` command

**`prune` mirrors `analyze`'s/`list`'s data-set-selection parameters** (the same
hard lockstep requirement) — it accepts `--repo`/`--context`/`--base`/`--engine`/
`--target-triple`/`--machine-key`/`--since`/`--until`/`--format`/
`--config`/`--verbose`, plus its own deletion-shaping flags: `--dirty`/`--clean`/
`--all` (scope, **required** — exactly one kind of run must be named), `--prune-base`
(base-branch confirmation), `--dry-run`, and `<commit>` subjects (repeatable bare
positional words, case-insensitive SHA-prefix). Instead of analyzing, it **deletes**
the selected objects.

**Deletion scopes** (`Scope::from_options`, fed from the clap `PruneCommitArgs`):
`--dirty` → dirty snapshots only; `--clean` → clean runs and their blessings only;
`--all` → both. A scope is **required** — the clap `ArgGroup("prune-kind")` is
`required(true)`, so a bare `prune` is rejected at parse time. `--all` expands in
`into_options` (`clean = self.clean || self.all`, `dirty = self.dirty || self.all`);
`Scope::from_options` maps `(clean, dirty)` → `All`/`Clean`/`Dirty` and errors on
`(false, false)` (defence in depth behind the clap group).

**Base-branch preservation (Design B) + the `--prune-base` guard.** Pruning never
touches base-branch history: `commit_is_eligible(index, merge_base_index,
tip_is_merge_base)` removes only commits **strictly after** the merge-base with
`--base` (the context branch's own commits); base-side commits are skipped with a
note. When the context resolves onto the base branch itself (`context == base`, i.e.
`tip_is_merge_base`), the whole selection *is* base-branch history, so `prune_with`
refuses with a `RunError::Analyze` naming the base branch unless `--prune-base`
confirms it (`resolve_base_name` resolves the branch name for the message, mirroring
`resolve_base_ref` precedence). There is **no** narrowing guard and no `--all`
override of one — the base-branch guard is the only protection against a large
accidental delete.

The one **intentional divergence** from `analyze`/`list`: the base-branch tip's
dirty runs are admitted **unconditionally** (`DirtyTipPolicy::Always`), whereas
`analyze`/`list` only admit them when the working tree is currently dirty
(`DirtyTipPolicy::WhenWorkingTreeDirty`). This is what lets `prune --dirty` reclaim
ephemeral base-branch snapshots regardless of the current tree state. The policy is
the parameter to the shared `resolve_history` helper in `analyze/mod.rs` (extracted
from `select_dataset`); `select_dataset` passes `WhenWorkingTreeDirty`, `prune`
passes `Always`.

`prune` lives **inside** the analyze module tree as `src/analyze/prune.rs`
(`pub(crate) mod prune;`), reusing the same `Selection` (built via `from_prune`,
which hardwires `no_dirty: false`), `parsed_facets`, `facet_filtered_candidates`,
and `resolve_history` pipeline. `prune_with` resolves the history, applies the
`--prune-base` guard, partitions the candidate objects into runs (clean/dirty, via
`RunKind`) and blessings, applies the eligibility / scope / `<commit>` /
`--since`/`--until` filters, then deletes in **two passes**: pass 1 removes the
selected runs (recording which clean `(set, commit)` pairs were removed); pass 2 —
only when the scope touches clean runs — removes a blessing sidecar iff its
`(set, commit)` had its clean run removed (so blessings follow their clean run and
are never time-filtered directly). `--since`/`--until` decide the window from the
git-topology `commit -> committer_time` map (`window_excludes`), keyed by each
candidate's commit SHA — no per-object body fetch. The deletions are grouped
into a `Plan` (by discriminant set, commits oldest-first by topology) and — unless
`--dry-run` — each key is removed via `Storage::delete`. The JSON report carries
`dry_run`, the per-commit run/blessing counts, and the deleted `keys` per commit;
text/markdown report counts only (the `verb` helper switches "Would remove" ↔
"Removed").

`Storage::delete(&self, key)` is on the `Storage` trait and all four impls
(Memory, Local, Azure, Facade); it returns `StorageError::NotFound`
when the object is absent (the local/memory impls validate the key first; Azure maps
a 404 / missing-container fault to `NotFound`).

## The `backfill` command

`commands::backfill` replays `run` across an inclusive commit range, bootstrapping
history for commits that predate the tool. The range endpoints are **positional
subjects** — `backfill <from> <to>` (oldest-first inclusive), not `--from`/`--to`
flags. It is generic over two ports so the
loop logic runs against Miri-safe fakes:

* `BackfillGit` — `resolve`/`first_parent` (topology, delegating to an embedded
  `SystemGitHistory`) plus the worktree lifecycle
  (`add_worktree`/`reset_to`/`remove_worktree`). The real `SystemBackfillGit`
  shells `git worktree add --detach --force`, `checkout --detach --force` +
  `reset --hard` + `clean -fd` (ignored build artifacts survive for incremental
  speed), and `worktree remove --force`. The fake `FakeBackfillGit` wraps
  `FakeGitHistory` and records `added`/`resets`/`removed` via `RefCell`.
* `CommitRunner` — `recorded_commits` (the set of commits already stored, probed
  once) plus `run` (runs and stores one already-checked-out commit). The real
  `SystemCommitRunner` reuses the `run` pipeline (`run::run_engines`) against a
  worktree-rooted `SystemProbe::in_dir` / `TokioBenchRunner::in_dir` /
  `FsBenchOutputSource` (`target_root = worktree/target`); the fake returns canned
  per-commit outcomes and a canned recorded set.

Key invariants:

* **The primary checkout is never mutated.** All work happens in a worktree under
  the system temp dir; config/project-id/storage are loaded once from the invoking
  checkout, never from the worktree. A dirty primary working tree is therefore
  fine — there is no clean-tree guard.
* **Validation precedes any worktree work** (`plan_commits`):
  require both endpoints to resolve and require the `<from>` subject to be a
  first-parent ancestor of the `<to>` subject. The range does **not** have to lie on
  `HEAD`'s history — any first-parent chain from `<from>` to `<to>` is backfillable.
  The range is enumerated oldest-first and inclusive via
  `first_parent(to).split_off(position(from))` (avoid `vec[a..]` — clippy
  `indexing_slicing`).
* **Pre-run existence check** (`run_commits`): in the default skip-existing mode,
  `recorded_commits` lists the project prefix (`v1/{project}/`) once and
  `commit_of_clean_key` extracts each clean object's commit segment; a commit in
  that set is reported `SkippedExisting` and its benchmark execution is skipped
  outright (no `reset_to`, no `run`). This makes backfill resumable without paying
  for already-benchmarked commits. The check is intentionally per-commit, not
  per-engine: a commit with a clean result for only some engines is still skipped —
  `--overwrite` clears the set so every commit is re-benchmarked and replaced.
* **Failure model** (`map_run_result`, a pure classifier): a stored set →
  `Stored{cases}`; an empty harvest → `SkippedEmpty`; `RunError::Duplicate` →
  `SkippedExisting` (a post-bench safety net for a commit not caught by the
  pre-check); `Engine`/`Command`/`Parse` → `BenchFailed` (a recoverable, per-commit
  failure that stops the loop unless `--ignore-errors`); every other `RunError`
  (storage, config, I/O) is **infrastructure** and aborts regardless of
  `--ignore-errors`. A stop surfaces as `RunError::Backfill` (a non-zero exit)
  carrying the partial summary.
* **Teardown always runs.** `execute_backfill` computes the `remove_worktree`
  result before propagating any per-commit error, so the worktree is removed on
  both the success and the error path.

Integration tests (`backfill_*`) drive the real adapters against a `clean_repo`
git tempdir: the mock engine's `--fail-if-exists PATH` flag exits non-zero when a
marker file is present in the checked-out worktree, so a commit that tracks the
marker stands in for one that fails to build. Helpers `commit_with_file` /
`commit_removing_file` / `make_dirty` build the needed histories.

These integration tests are deliberately kept to a small real-adapter smoke
subset (a clean-object success across a range, a merge-spanning range, and a
stop-on-failure drive) using **minimal commit ranges**. The planning, skip,
overwrite, error-handling, and reporting logic is exhaustively covered by the
fake-driven unit tests in `commands/backfill.rs` (over `FakeBackfillGit` /
`FakeCommitRunner` / `MemoryStorage`), so do not re-prove that logic with extra
real-git drives — each one spawns many git subprocesses and is the dominant cost
under mutation testing. Add coverage at the fake level instead.

## Engine adapters (Callgrind, Criterion, `alloc_tracker`, `all_the_time`)

Four benchmark engines are supported, each behind a pure parser in `bench/`:

* `bench::callgrind` parses a Gungraun v6 `summary.json` into a `BenchmarkResult`
  (instruction count, cache hits, estimated cycles, branches). `package` comes
  from the `package_dir` basename. Hardware-independent → stored under the
  `synthetic` partition segment, no machine key.
* `bench::criterion` parses a `new/benchmark.json` + `new/estimates.json` pair
  into a `WallTime` `BenchmarkResult`. The metric value is the `slope` point
  estimate when present (linear `b.iter` sampling) else the `mean`; the std-dev
  and the chosen estimate's CI bounds are recorded on the `Metric` (dispersion
  fields) so later analysis can be noise-aware. The identity is
  `group_id`/`function_id`/`value_str`, and **`package` is `None`** — Criterion
  files carry no package and `target/criterion/` is flat, so it is unrecoverable;
  the workspace's crate-prefixed group ids keep series distinct anyway. Criterion
  is hardware-dependent → partitioned by the host triple and a machine key.
* `bench::alloc_tracker` parses a flat `target/alloc_tracker/<operation>.json`
  file (one per operation, auto-emitted on `Session` drop) into a `BenchmarkResult`
  with an `AllocatedBytes` (`allocated_bytes`) and an `AllocationCount`
  (`allocations`) metric, both from the `mean_*_per_iteration` fields. `package`
  is `None`; the operation name is the identity. Allocation behaviour is a
  deterministic property of the code → `synthetic` partition, no machine key, no
  dispersion (treated like Callgrind for change-point detection).
* `bench::all_the_time` parses a flat `target/all_the_time/<operation>.json` file
  into a `BenchmarkResult` with a single `ProcessorTime` (`processor_time`, unit
  `ns`) metric. It prefers the through-origin `slope_processor_time_nanos` as the
  point estimate (falling back to `mean_processor_time_nanos`) and records the
  bootstrap confidence interval and standard deviation on the metric, mirroring
  the Criterion adapter. `package` is `None`. Processor time is hardware-dependent
  and noisy → partitioned by the host triple and a machine key; because it now
  carries a CI, the noise detector applies the CI-non-overlap gate to it (older
  mean-only output without an interval falls back to the
  Mann-Whitney/Mann-Kendall tests).

There is no engine configuration. `run` and `backfill` invoke `cargo bench` once
with the combined environment every supported engine needs
(`injected_bench_env`, today `GUNGRAUN_SAVE_SUMMARY=pretty-json`; the other three
engines auto-emit JSON on drop and need no env), then harvest every output tree
(`target/criterion/**`, `target/gungraun/**`, `target/alloc_tracker/*.json`,
`target/all_the_time/*.json`); an engine that produced no cases (for example
Callgrind off Linux, where the `_cg` benches are `#[cfg(target_os = "linux")]`
no-ops) is silently skipped. Scope a run with `--workspace` (the default),
`--package`/`-p NAME` (repeatable), or `--bench NAME` (repeatable) — these
translate to the matching `cargo bench` arguments. `--engine` is **not** a
`run`/`backfill` flag; it is an `analyze` facet over already-stored data.
`Engine::ALL` enumerates the engines harvested per run.

## Machine key

`machine::system_profile` reads the `many_cpus` processor and memory-region
counts plus a best-effort, per-platform `detect_cpu_brand`. `machine::fingerprint`
hashes a version-tagged canonical string (`mk1\nprocessors=…\nmemory_regions=…\n
cpu_brand=…`) with SHA-256 and truncates to the first 16 hex chars — a **fixed**
(not seeded) hash so the key is stable across machines and tool versions; a golden
test pins a fixed profile to its digest. `resolve_machine_key` prefers an explicit
`--machine-key` override (a CLI-only flag — the config file is committed, so it must
not carry a machine key that would be wrong for some checkouts) over the
fingerprint. The key is
computed only when `engine.is_hardware_dependent()` (Criterion, `all_the_time`);
Callgrind and `alloc_tracker` never read it.

All time comes from an injected `tick::Clock`. Production uses
`Clock::new_tokio()`; tests use `Clock::new_frozen_at(...)` for deterministic
keys and timestamps. Do not call `SystemTime::now()` in orchestration code and
never insert real-time delays in tests.

## Miri strategy

* Pure logic and fake-driven orchestration tests are plain `#[test]` using
  `futures::executor::block_on` plus a frozen clock — no Tokio runtime, no IO —
  so they run under Miri.
* Any test that touches the real filesystem, spawns a process, starts a Tokio
  runtime, or reads the wall clock must be `#[tokio::test]` (or `#[test]` for
  `std::fs`) **and** `#[cfg_attr(miri, ignore = "…")]` with a reason. This covers
  the real-IO end-to-end tests, the Azurite network tests, and the account-key
  `AzureBlobStorage` tests (building an account SAS reads the clock for its
  expiry; the pure SAS signing math is still covered under Miri by the
  fixed-expiry golden vector in `storage::sas`).

## Mock engine for end-to-end tests

The integration tests drive `run` against the **real** process/filesystem/storage
adapters, so they need a real program to launch as the "engine". That program is
`src/bin/cargo-bench-history-mock-engine.rs`, an auto-discovered binary target
(`cargo-bench-history-mock-engine`); tests reference it via
`env!("CARGO_BIN_EXE_cargo-bench-history-mock-engine")`. It writes the committed
Gungraun summary fixtures into `<target-root>/gungraun/GROUP/summary.json` (so the
harvester finds fresh output) and exits with a chosen code — never use a shell
builtin like `exit 7`, because the runner no longer goes through a shell. Resolve
`<target-root>` the same way the harvester does (`CARGO_TARGET_DIR` or `target`),
so the mock writes where the harvester reads. The integration harness injects the
mock as the benchmark command via the `run_with_overrides` `bench_command`
override (`[MOCK_ENGINE] + self.bench`, set with `Workspace::with_bench`), so the
program path and each fixture-describing argument are passed verbatim as distinct
argv entries — no shell, no quoting, no config `command`. After the mock's own
contiguous arguments, `run` appends the cargo scope flags (`--workspace`/
`--package NAME`/`--bench NAME`) and any `--` passthrough, which the mock ignores
(it stops at the first argument it does not recognize). Like every other crate
root that uses `#[cfg_attr(coverage_nightly, coverage(off))]`, the mock engine
must declare `#![cfg_attr(coverage_nightly, feature(coverage_attribute))]` at its
top, or the coverage job fails to compile it (E0658).

The mock engine accepts `--summary GROUP=KIND` (repeatable). `KIND` is `single`
(unparametrized, `Ir` = 36, package `fast_time`), `parametrized` (id
`two_instants`, `Ir` = 87), or `single-alt-pkg` — the `single` fixture with its
`package_dir` swapped for a different package while the `module_path`/
`function_name` stay identical. `single-alt-pkg` exists to simulate two equally
named bench targets in different packages: their `BenchmarkId`s differ only in
`package`, so they must stay distinct series rather than silently merge. If the
parser ever stops folding `package_dir` into the identity, the
`run_distinguishes_same_module_path_across_packages` test fails. `GROUP` may
contain `/`, which splits into nested directories under `gungraun/` so a test can
reproduce a real engine's per-binary nesting (and the on-disk collision of two
same-named bench binaries in different packages, exercised by
`run_harvests_colliding_bench_binary_names_in_distinct_packages`).

It also accepts `--criterion GROUP|FUNCTION|VALUE=NANOS` (repeatable), which
writes a Criterion `new/benchmark.json` + `new/estimates.json` pair under
`<target-root>/criterion/<group>/<function>[/<value>]/new/` whose `slope` point
estimate is `NANOS` nanoseconds (an empty `VALUE` omits the parameter component).
Distinct identities never share a directory, so one run can emit several Criterion
cases that harvest into one result set as separate records.

It accepts `--alloc-tracker OPERATION=BYTES/COUNT` (repeatable), which writes a
flat `<target-root>/alloc_tracker/<OPERATION>.json` reporting `BYTES` mean bytes
and `COUNT` mean allocations per iteration, and
`--all-the-time OPERATION=NANOS[@LOW:HIGH]` (repeatable), which writes a flat
`<target-root>/all_the_time/<OPERATION>.json` reporting `NANOS` mean (and slope)
processor-time nanoseconds per iteration. An optional `@LOW:HIGH` suffix also
records a bootstrap confidence interval (plus a matching standard deviation, span
count and min/max), exercising the adapter's dispersion path and the noise
detector's CI-non-overlap gate on processor time. Both mirror the real crates'
one-file-per-operation auto-emitted output.

Finally, `--fail-if-exists PATH` exits with code 1 and writes no output when
`PATH` (relative to the working directory) exists. Backfill runs each engine in a
checked-out worktree, so a commit that tracks the named marker file stands in for
a commit that fails to build or benchmark; the `backfill_*` integration tests use
this to exercise the stop-on-failure and `--ignore-errors` paths.

## End-to-end tests and the current directory

The real-adapter end-to-end test changes the process current directory into a
tempdir, so it must be `#[serial]` (from `serial_test`). On Windows a `TempDir`
cannot be dropped while it is the current directory, so restore the original CWD
**before** running assertions and before the tempdir is dropped. Prefer setting
`[project] id` in the test config rather than relying on the CWD basename.

Each test also points the harvest at its own tempdir `target/` for the duration
of `run`, and drives `run`/`backfill` against the mock benchmark program instead
of `cargo bench`, by passing both an explicit target root and the benchmark
command through the `run_with_overrides` entry. `run` injects that resolved root
into the benchmark environment as `CARGO_TARGET_DIR`, so the engine writes exactly
where the harvest scans. This matters under the coverage job, which runs `cargo
llvm-cov nextest` and redirects every build and bin into one shared
`target/llvm-cov-target`: that ambient `CARGO_TARGET_DIR` would otherwise make the
mock engine (which honors it) write its summaries into the shared directory while
the overridden harvester looked in the tempdir, harvesting nothing. Pinning both
ends to the same root keeps each test hermetic **without mutating the process
environment** — do not reintroduce a `set_var("CARGO_TARGET_DIR", …)` guard. The
production `run` entry passes `None` for both, resolves the root from the
environment as usual, runs `cargo bench`, and injects that same value so the real
engine and harvest never diverge either.

Because that override pins an absolute root and runs the mock from the workspace
root, it cannot catch a bug in `resolve_target_root` itself. The resolved root
must be **absolute**: cargo runs each `--package`-scoped benchmark binary with its
working directory set to the owning package's directory, so a relative
`CARGO_TARGET_DIR` would be resolved there by an engine that honors it (Criterion),
scattering output away from the workspace-rooted harvest — the "Stored 0 result
sets" symptom. `run_harvests_output_when_the_engine_runs_in_a_package_directory`
guards this by driving through `drive_resolving_target_root` (no override, so the
real `resolve_target_root` runs) with the mock's `--chdir` flag standing in for
that per-package cwd. Keep both: a regression in `resolve_target_root` would slip
past every override-driven test.

`tests/cbh_real_engine.rs` is the one true end-to-end test: it
writes a tiny standalone crate with a single real Criterion benchmark into a
tempdir and drives the production `run` (no overrides) against an actual `cargo
bench`, asserting the genuine wall-time output is harvested and stored. It is the
only test that exercises `resolve_target_root`, real cargo argument passing, and
Criterion's real on-disk layout together, so it would have caught the relative
`CARGO_TARGET_DIR` bug on its own. It is deliberately heavyweight (it compiles
Criterion and runs a benchmark process), so it is gated off under Miri and under
coverage (`#[cfg_attr(coverage_nightly, ignore)]`); the benchmark is shrunk to
Criterion's smallest run (10 samples, sub-second warm-up/measurement) and
Criterion is taken with `default-features = false` to keep the build quick. When
touching the run/harvest pipeline, do not let this test's gating tempt you to skip
it locally — run it with plain `cargo test`/`nextest` (it is not Miri- or
coverage-gated there). It is additionally `#[cfg_attr(mutants, ignore)]`: the
mock-engine integration tests and the `resolve_target_root` unit test cover the same
pipeline under mutation, so this heavyweight build (it recompiles Criterion on every
mutant) carries no unique mutation signal — see the Mutation testing section.

## Fixture-golden canaries

`tests/fixtures/callgrind/*.summary.json` are **real** Gungraun output,
`tests/fixtures/criterion/*/{benchmark,estimates}.json` are **real** Criterion
output, and `tests/fixtures/alloc_tracker/*.json` + `tests/fixtures/all_the_time/
*.json` are **real** `alloc_tracker` / `all_the_time` output, all committed
verbatim. They are schema-drift canaries used by the parser
tests, the orchestration tests, and the integration test. Do not hand-edit them to
make a test pass — regenerate the Callgrind fixtures from `just bench-cg` (and the
Criterion / `alloc_tracker` / `all_the_time` fixtures from a `cargo bench` run) if
the upstream schema genuinely changes, and update the parser/tests to match.

## Azure Blob storage

`AzureBlobStorage` is a `Storage` backed by an Azure Blob container. It is
**always compiled in** — `cargo-bench-history` is a CLI tool that users install
without choosing feature flags, so gating the backend behind a build feature
would only ever hide a backend the user explicitly configured. It pulls in the
Azure SDK and the RustCrypto `hmac`/`sha2` crates used to self-sign SAS tokens.
Object keys map 1:1 to `/`-separated blob names, so the key model is identical to
`LocalStorage` (write-once, list-by-prefix).

Authentication is resolved once in `AzureBlobStorage::from_config`, in priority
order: a self-signed account SAS (`account_key`), a verbatim `sas_token`, or
Microsoft Entra ID (`DeveloperToolsCredential`). SAS modes carry the token in the
endpoint URL's query and pass no credential, so the emulator's plain-HTTP
endpoint is accepted; Entra mode passes a token credential and requires HTTPS.
The Entra credential is wrapped in a `CachingCredential` decorator that holds a
cache lock across the inner acquisition, so a concurrent read burst collapses to
one `az account get-access-token` call rather than racing the Windows MSAL
token-cache lockfile (which fails the read with a permission error).
`CachingCredential` implements the azure SDK's `#[async_trait]` `TokenCredential`
trait, so it is the one place the crate pulls in `async_trait` — the crate's own
IO ports still use RPITIT (`impl Future`), as the external trait leaves no choice.
The account-SAS signer lives in `storage::sas` and is verified against a pinned
golden signature — do not "fix" that test by editing the expected value.

### Running the Azurite tests locally

The network tests (`storage::azure::tests` and the `cbh_azure`
integration file) talk to a live [Azurite](https://github.com/Azure/Azurite)
blob emulator. They **self-skip** when no emulator is reachable, so a normal test
run stays green without one; they run for real once Azurite is up.

The `just test-azurite` recipe wraps the whole flow. Install the emulator once with
`just install-tools` (or manually with `npm install -g azurite`), then:

```powershell
just test-azurite
```

It starts an in-memory Azurite on `127.0.0.1:10000` (reusing one already running),
runs the Azure-backend tests with `BENCH_HISTORY_REQUIRE_AZURITE=1` so an
unreachable emulator is a hard error rather than a silent skip, and stops the
emulator it started afterward — even on failure (it never stops one it did not
start). It is the local counterpart of `just test-azure`. To run the tests by hand
instead, start the emulator and point `cargo` at it:

```powershell
# On Windows azurite-blob is a .cmd, so launch it through cmd:
cmd /c azurite-blob --blobHost 127.0.0.1 --blobPort 10000 --inMemoryPersistence --skipApiVersionCheck --silent --loose
# then, in another shell:
cargo test -p cargo-bench-history
```

* `AZURITE_BLOB_ENDPOINT` overrides the default
  `http://127.0.0.1:10000/devstoreaccount1` endpoint.
* `BENCH_HISTORY_REQUIRE_AZURITE=1` turns an unreachable emulator into a hard
  failure instead of a skip. `just test-azurite` sets it for you; CI also sets it
  in the dedicated `test-azurite` job so a misconfigured emulator can never
  silently degrade into skipping every test.

In CI these tests run only in the Linux-only `test-azurite` job (see
`.github/workflows/validation.yml`), which starts Azurite on the runner host via
the `start-azurite` composite action and also collects coverage so the
`azure.rs` network paths reach Codecov (the multi-platform `coverage` job runs
without an emulator and so cannot cover them).

### Running the real-Azure tests locally

The `*_in_real_azure` tests in `cbh_azure` exercise the **Microsoft Entra ID**
path against a real Storage account (the production default that Azurite's
account-SAS path never touches). They **self-skip** unless `ENABLE_AZURE` is set —
an explicit opt-in, so the account name living in `constants.env` does not by itself
make a plain test run target the cloud. Each test uses a fresh container that is
deleted when it finishes, even on panic.

One-time: deploy the account, managed identity and federated credentials with the
Bicep + scripts in [`infra/azure-bench-history/`](../../infra/azure-bench-history/)
(see its README) and record the identifiers in the repository-root `constants.env`.
Then, signing in as your own Entra user, the `just test-azure` recipe wraps the run
(it sets `ENABLE_AZURE` and reads the account from `constants.env` for you):

```powershell
az login
just test-azure
# tidy up any container a crashed run left behind:
./infra/azure-bench-history/cleanup-containers.ps1
```

`just test-azure` reads the account from `BENCH_HISTORY_AZURE_ACCOUNT` (committed in
`constants.env`); pass a name to target a different account
(`just test-azure <storage-account-name>`). The equivalent raw command is
`cargo test -p cargo-bench-history real_azure` with `ENABLE_AZURE`
and `BENCH_HISTORY_AZURE_ACCOUNT` set.

* `BENCH_HISTORY_AZURE_ENDPOINT` overrides the default
  `https://<account>.blob.core.windows.net` endpoint.
* `ENABLE_AZURE` is the gate: when set, the real-Azure tests run, and a then-missing
  `BENCH_HISTORY_AZURE_ACCOUNT` is a hard failure instead of a skip. `just test-azure`
  sets it locally; CI sets it (via that recipe) in the dedicated `test-azure` job,
  which signs in via GitHub OIDC workload identity federation, so it can never
  silently skip. It is deliberately NOT in `constants.env`, so a plain `just test`
  leaves the real-Azure tests skipped.

In CI these tests run only in the Linux-only, same-repo-only `test-azure` job (see
`.github/workflows/validation.yml` and `.github/workflows/AGENTS.md`). It collects
no coverage — `test-azurite` already covers `azure.rs`; its value is proving the
real Entra + real Blob endpoint round-trip.

### Mutation testing

The `mutants` CI jobs run without Azurite, so the SDK-delegating IO methods
(`put`/`put_overwrite`/`get`/`list`/`delete`/`ensure_container` plus the
`upload_with_retry` and `upload` helpers) — whose only coverage is the Azurite
round-trip tests — carry `#[cfg_attr(test, mutants::skip)]`, the same pattern
`probe.rs` uses for its `git`/`rustc` shell-outs. The pure logic those methods
lean on (`classify`, `map_error`, the `storage::sas` signer, and
`account_sas_expiry`) has dedicated unit tests and stays under mutation testing,
so the error-mapping and signing behavior is still mutation-covered.

**Skipping non-discriminating tests under mutation.** cargo-mutants builds with
`--cfg mutants` set, so a test that carries no mutation signal here can opt out at
the test site with `#[cfg_attr(mutants, ignore = "<why>")]`. Unlike `mutants::skip`
on production code, this is self-documenting where the test is defined and leaves
normal `cargo test`, coverage, and the `test-azurite`/`test-azure` jobs untouched —
the test still compiles and runs everywhere except under mutation. The Azurite and
real-Azure network tests (`storage::azure::tests` round-trips and the whole
`cbh_azure` suite) use this: without an emulator they only self-skip — slowly,
through a TCP connect timeout — while the IO they would cover is already
`mutants::skip`. `tests/cbh_real_engine.rs` uses it too: its run/harvest pipeline is
also driven by the mock-engine integration tests and the `resolve_target_root` unit
test, so it adds no unique mutation signal yet would recompile Criterion on every
mutant. The `cfg(mutants)` name is registered in the workspace `unexpected_cfgs`
`check-cfg` list so the zero-warnings policy still holds.

## Stress harness (`cargo-bench-history-stress`)

The sibling `cargo-bench-history-stress` package (under `packages/`) is an
**on-demand** binary that fabricates a giant synthetic benchmark history, seeds it
into a storage backend, and times each `analyze` mode (`history`, `branch`, `tip`)
over it. Its purpose is to observe how `analyze` scales; a full-scale run is on-demand
only and never runs automatically in `just test` or CI. The package's own small unit
and integration tests do run as a normal workspace member (and are subject to mutation
testing and coverage). See that package's `README.md` for the dataset shape and flags.

Key facts when touching it:

* **Zero production-code coupling.** The harness only *writes* objects in the same
  key layout the storage backends use (`v1/{project}/{engine}/{triple}/{machine}/
  {commit}/{file}`) and in the same **gzip body encoding** — it calls
  `cargo_bench_history_core::codec::compress` (the exact function the backends use)
  rather than reimplementing it, and `seed.rs` reports the *compressed* byte length
  so the total-bytes figure reflects real on-disk/wire volume. It then reads the
  objects back through the real public `cargo_bench_history::run_with_overrides`
  entry point, which inflates them transparently. It deliberately does **not**
  reach into private storage internals, so it needs no `_impl`-crate split and no
  test-only feature on the shell crate. If a refactor changes the on-disk key layout
  or the JSON report's top-level fields, the harness's `seed.rs` / `report.rs` must
  be updated in lockstep.
* **Storage selection** mirrors the tests: local filesystem by default, Azure Blob
  under `--storage azure` (the Azure backend is always compiled in). Azure upload
  uses `azcopy` (installed by `just install-tools`)
  for throughput, authenticating as the Entra user via the Azure CLI — same
  `az login` + `BENCH_HISTORY_AZURE_ACCOUNT` contract as `test-azure`. Each run uses
  a fresh `bh-stress-<unix>` container, deleted on exit unless `--keep`.
* **Run it** with `just bench-history-stress` (local) or `just bench-history-stress-azure`
  (real Azure); both pass extra flags through to the binary, e.g.
  `just bench-history-stress --commits 100`.
* **Determinism.** A given `--seed` and sizing reproduce a byte-identical dataset
  (fixed dataset anchor + SplitMix64 value generator), so timings are comparable
  across runs. The synthetic value model is shaped so each mode reports a sensible,
  explainable mix of regressions/improvements rather than all-or-nothing (see the
  family/divisor comments in `scenario.rs`).
* **Dataset shape.** The default fabricates 1000 benchmarks × 2000 `main` commits
  (only ~half store a run — the rest are gaps that exercise the "commit with no run"
  path) across **all** engines crossed with the platforms they run on (`callgrind`
  Linux-only; `criterion`/`alloc_tracker`/`all_the_time` on all six triples), for 20
  discriminant sets. Deterministic engines (`callgrind`, `alloc_tracker`) store exact
  values with no dispersion; noisy engines (`criterion`, `all_the_time`) store the
  same shape plus a tight confidence band — kept well below the injected step
  magnitudes — so both the exact and the noise-aware detection paths are exercised.
  `scenario::metric_for` picks the metric kind and band per engine.

