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
  `FakeProbe` returns canned `GitSnapshot`/`RustcInfo`/`HardwareProfile`.
* `bench_output::BenchOutputSource` — real `FsBenchOutputSource` walks the
  engine's output tree and returns a `Harvest` enum: `Harvest::Callgrind` from
  `target/gungraun/**/summary.json` or `Harvest::Criterion` from
  `target/criterion/**/new/{benchmark,estimates}.json` (only `new/` dirs holding
  both files, `mtime`-scoped to the run). Fake `FakeOutput` is engine-aware and
  returns in-memory `RawSummary` or `RawCriterionCase` values.
* `storage::Storage` — real `LocalStorage` and (behind the `azure` feature)
  `AzureBlobStorage`, selected at runtime by the `StorageFacade` enum that
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
filters, the resolved target/base/merge-base, and why each candidate object is
included or excluded; `install` notes whether it wrote or left the config. The
reporter is `&dyn Reporter` (not `+ Sync`), so the run futures stay `!Send` and
Miri-driven via `block_on`.

`analyze` also renders a non-verbose diagnostic *hint* (carried on `ReportInput`/
`JsonReport`, built by `empty_history_hint`) whenever facet-matching runs were
stored but none entered the analysis — most commonly when every run is a dirty
snapshot on a base-side commit (the "config file never committed" trap). The hint
makes a `0 runs` result self-explanatory without needing `--verbose`.

## Storage model (commit-centric v2)

`comparability::ComparabilityKey` builds object keys under the partition prefix
`v2/<project>/<engine>/<triple>/<machine|synthetic>` and then keys each point by
**commit**:

* `clean_key(commit)` → `…/<commit>/clean.json` — the one canonical point for a
  clean working tree at that commit. It is deterministic, so a re-run of the same
  commit maps to the same key; the write-once `put` turns that into a
  `RunError::Duplicate` (refused) unless `--overwrite` switches to `put_overwrite`.
* `dirty_key(commit, effective_unix)` → `…/<commit>/dirty-<effective_unix>.json` —
  a snapshot of an uncommitted tree, distinguished by its effective second so
  multiple dirty snapshots on one base commit coexist; only a same-second clash is
  a conflict.

The commit segment is the **full** SHA (`git.info.commit`, `unknown` when there is
no repo), because the git-aware `analyze` reads `v2/.../<full_sha>/` directories
resolved from `git rev-list`. The `run` store step picks clean vs dirty from
`git.info.dirty`; clean effective time defaults to the committer date, dirty to
wall-clock now, and `--timestamp` overrides either. There is no run-id in the key.

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
  `v2/<project>/<engine>/<triple>/<machine|synthetic>/<commit>/<file>` key into a
  `DiscriminantSet` (engine, triple, os/arch derived from the triple, machine).
  `--engine`/`--os`/`--architecture`/`--machine-key` select sets (case-insensitive
  facet match); each surviving set becomes its own sub-report. `--target-triple`
  matches the whole triple directly and is mutually exclusive with `--os` /
  `--architecture` (the triple fixes both — `validate_triple_exclusivity` rejects
  the combination). `--list-discriminants`
  prints the present sets and returns **without requiring a repository** (it is a
  pure index over storage keys).
* **Repository required for analysis.** Resolving a timeline needs git, so when not
  just listing discriminants `analyze` errors if no repository resolves. The target
  ref is `--branch` (default `HEAD`); the base ref is `--base` >
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
  commit by `(dirty, effective, object key)` so a clean point precedes same-commit
  dirty snapshots deterministically. Topology — not effective time — is primary, so
  back-dated backfill runs still sort by where their commit sits in history.
  `--since` filters at the object level — whole runs before the cutoff are dropped.
* `analyze::findings` is the rolling-baseline regression detector (median
  baseline over a bounded window, MAD-aware threshold, severity tiers). Keep it
  deterministic and cover boundaries with named value-asserting tests, not
  threshold guards.
* `analyze::report` renders text/json/markdown. The top-level aggregate carries a
  `sets` array (one entry per discriminant set). Rendering is infallible: the
  report is plain structs of finite numbers, so the JSON path uses `.expect`
  rather than threading a serialization error nobody can trigger.

The read-only `git_history::GitHistory` port (`resolve`, `default_branch`,
`merge_base`, `first_parent`) has a `SystemGitHistory` adapter that shells
`git -C <repo>` and a `#[cfg(test)]` `FakeGitHistory` over a canned commit graph.
Integration tests build a **real** git repo in a tempdir (`Workspace::repo` /
`Workspace::clean_repo`) and seed storage keys under the commits' real SHAs so the
live topology and the stored keys agree.

## The `backfill` command

`commands::backfill` replays `run` across an inclusive commit range, bootstrapping
history for commits that predate the tool. It is generic over two ports so the
loop logic runs against Miri-safe fakes:

* `BackfillGit` — `is_clean`/`resolve`/`first_parent` (topology, the latter two
  delegating to an embedded `SystemGitHistory`) plus the worktree lifecycle
  (`add_worktree`/`reset_to`/`remove_worktree`). The real `SystemBackfillGit`
  shells `git worktree add --detach --force`, `checkout --detach --force` +
  `reset --hard` + `clean -fd` (ignored build artifacts survive for incremental
  speed), and `worktree remove --force`. The fake `FakeBackfillGit` wraps
  `FakeGitHistory` and records `added`/`resets`/`removed` via `RefCell`.
* `CommitRunner` — runs and stores one already-checked-out commit. The real
  `SystemCommitRunner` reuses the `run` pipeline (`run::run_engines`) against a
  worktree-rooted `SystemProbe::in_dir` / `TokioBenchRunner::in_dir` /
  `FsBenchOutputSource` (`target_root = worktree/target`); the fake returns canned
  per-commit outcomes.

Key invariants:

* **The primary checkout is never mutated.** All work happens in a worktree under
  the system temp dir; config/project-id/storage are loaded once from the invoking
  checkout, never from the worktree.
* **Validation precedes any worktree work** (`plan_commits`):
  require both endpoints to resolve, require `--from` to be a first-parent ancestor
  of `--to`, and require `--to` to be on `HEAD`'s first-parent history (so the
  points are later analyzable). The range is enumerated oldest-first and inclusive
  via `first_parent(to).split_off(position(from))` (avoid `vec[a..]` — clippy
  `indexing_slicing`).
* **Failure model** (`map_run_result`, a pure classifier): a stored set →
  `Stored{cases}`; an empty harvest → `SkippedEmpty`; `RunError::Duplicate` →
  `SkippedExisting` (resumable); `Engine`/`Command`/`Parse` → `BenchFailed` (a
  recoverable, per-commit failure that stops the loop unless `--ignore-errors`);
  every other `RunError` (storage, config, I/O) is **infrastructure** and aborts
  regardless of `--ignore-errors`. A stop surfaces as `RunError::Backfill` (a
  non-zero exit) carrying the partial summary.
* **Teardown always runs.** `execute_backfill` computes the `remove_worktree`
  result before propagating any per-commit error, so the worktree is removed on
  both the success and the error path.
* **Multi-engine collision caveat:** if one engine stores and another is a
  duplicate, `run_engines` returns `Err(Duplicate)` and the whole commit is
  reported `SkippedExisting`; `--overwrite` avoids it.

Integration tests (`backfill_*`) drive the real adapters against a `clean_repo`
git tempdir: the mock engine's `--fail-if-exists PATH` flag exits non-zero when a
marker file is present in the checked-out worktree, so a commit that tracks the
marker stands in for one that fails to build. Helpers `commit_with_file` /
`commit_removing_file` / `make_dirty` build the needed histories.

## Engine adapters (Callgrind and Criterion)

Two benchmark engines are supported, each behind a pure parser in `bench/`:

* `bench::callgrind` parses a Gungraun v6 `summary.json` into a `ResultRecord`
  (instruction count, cache hits, estimated cycles, branches). `package` comes
  from the `package_dir` basename. Hardware-independent → stored under the
  `synthetic` partition segment, no machine key.
* `bench::criterion` parses a `new/benchmark.json` + `new/estimates.json` pair
  into a `WallTime` `ResultRecord`. The metric value is the `slope` point
  estimate when present (linear `b.iter` sampling) else the `mean`; the std-dev
  and the chosen estimate's CI bounds are recorded on the `Metric` (dispersion
  fields) so later analysis can be noise-aware. The identity is
  `group_id`/`function_id`/`value_str`, and **`package` is `None`** — Criterion
  files carry no package and `target/criterion/` is flat, so it is unrecoverable;
  the workspace's crate-prefixed group ids keep series distinct anyway. Criterion
  is hardware-dependent → partitioned by the host triple and a machine key.

There is no engine configuration. `run` and `backfill` invoke `cargo bench` once
with the combined environment every supported engine needs
(`injected_bench_env`, today `GUNGRAUN_SAVE_SUMMARY=pretty-json`), then harvest
both output trees (`target/criterion/**`, `target/gungraun/**`); an engine that
produced no cases (for example Callgrind off Linux, where the `_cg` benches are
`#[cfg(target_os = "linux")]` no-ops) is silently skipped. Scope a run with
`--workspace` (the default), `--package`/`-p NAME` (repeatable), or `--bench NAME`
(repeatable) — these translate to the matching `cargo bench` arguments. `--engine`
is **not** a `run`/`backfill` flag; it is an `analyze` facet over already-stored
data. `EngineSystem::ALL` enumerates the engines harvested per run.

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
computed only when `engine.is_hardware_dependent()` (Criterion); Callgrind never
reads it.

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
`tests/support/mock_engine.rs`, declared as a `[[bin]]`
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

`tests/cargo_bench_history_real_engine.rs` is the one true end-to-end test: it
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
coverage-gated there).

## Fixture-golden canaries

`tests/fixtures/callgrind/*.summary.json` are **real** Gungraun output and
`tests/fixtures/criterion/*/{benchmark,estimates}.json` are **real** Criterion
output, both committed verbatim. They are schema-drift canaries used by the parser
tests, the orchestration tests, and the integration test. Do not hand-edit them to
make a test pass — regenerate the Callgrind fixtures from `just bench-cg` (and the
Criterion fixtures from a `cargo bench` run) if the upstream schema genuinely
changes, and update the parser/tests to match.

## Azure Blob storage (`azure` feature)

The `azure` feature adds `AzureBlobStorage`, a `Storage` backed by an Azure Blob
container. It is **off by default** so the common build stays light; enabling it
pulls in the Azure SDK and the RustCrypto `hmac`/`sha2` crates used to self-sign
SAS tokens. Object keys map 1:1 to `/`-separated blob names, so the key model is
identical to `LocalStorage` (write-once, list-by-prefix).

Authentication is resolved once in `AzureBlobStorage::from_config`, in priority
order: a self-signed account SAS (`account_key`), a verbatim `sas_token`, or
Microsoft Entra ID (`DeveloperToolsCredential`). SAS modes carry the token in the
endpoint URL's query and pass no credential, so the emulator's plain-HTTP
endpoint is accepted; Entra mode passes a token credential and requires HTTPS.
The account-SAS signer lives in `storage::sas` and is verified against a pinned
golden signature — do not "fix" that test by editing the expected value.

### Running the Azurite tests locally

The network tests (`storage::azure::tests` and the `cargo_bench_history_azure`
integration file) talk to a live [Azurite](https://github.com/Azure/Azurite)
blob emulator. They **self-skip** when no emulator is reachable, so a normal
`--all-features` run stays green without one; they run for real once Azurite is
up. To run them:

```powershell
npm install -g azurite
# On Windows azurite-blob is a .cmd, so launch it through cmd:
cmd /c azurite-blob --blobHost 127.0.0.1 --blobPort 10000 --inMemoryPersistence --skipApiVersionCheck --silent --loose
# then, in another shell:
cargo test -p cargo-bench-history --features azure
```

* `AZURITE_BLOB_ENDPOINT` overrides the default
  `http://127.0.0.1:10000/devstoreaccount1` endpoint.
* `BENCH_HISTORY_REQUIRE_AZURITE=1` turns an unreachable emulator into a hard
  failure instead of a skip. CI sets it in the dedicated `test-azure` job so a
  misconfigured emulator can never silently degrade into skipping every test.

In CI these tests run only in the Linux-only `test-azure` job (see
`.github/workflows/validation.yml`), which starts Azurite on the runner host via
the `start-azurite` composite action and also collects coverage so the
`azure.rs` network paths reach Codecov (the multi-platform `coverage` job runs
without an emulator and so cannot cover them).

### Mutation testing

The `mutants` CI jobs run without Azurite, so the SDK-delegating IO methods
(`put`/`get`/`list`/`ensure_container`/the `upload` helper) — whose only coverage
is the Azurite round-trip tests — carry `#[cfg_attr(test, mutants::skip)]`, the
same pattern `probe.rs` uses for its `git`/`rustc` shell-outs. The pure logic
those methods lean on (`classify`, `map_error`, the `storage::sas` signer, and
`account_sas_expiry`) has dedicated unit tests and stays under mutation testing,
so the error-mapping and signing behavior is still mutation-covered.
