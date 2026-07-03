# Agent notes for cargo-bench-history

An async-by-default Tokio application. **All pure logic** (parsing, mapping, comparability,
key derivation, analysis, message formatting) stays **synchronous**; async is pushed only to
the IO edges. The code is **two crates**: this shell (CLI + IO adapters) and the pure,
I/O-free `cargo-bench-history-core` (data model, analysis, codec), which also acts as the
impl crate for in-workspace tests/benches.

> **This file is agent instructions, not a design doc.** The *what* and *why* — data model,
> storage/comparability rules, command semantics, analysis algorithms, architecture — live in
> [`docs/DESIGN.md`](docs/DESIGN.md), and the `analyze` load/detect flow in
> [`docs/analyze.md`](docs/analyze.md). **Keep both in sync** when you change a design
> decision, the data model, the crate layout, the storage format, a command's behaviour, or
> the analyze pipeline. Do not restate their content here — point at them.

## Ports and fakes

Every IO edge is a small **port trait** (RPITIT — `impl Future` return, no `async_trait`)
with a real Tokio adapter and an in-`#[cfg(test)]` in-memory fake, so orchestration runs
against fakes under `block_on` (Miri-safe, no runtime). The ports:

* `process::BenchRunner` — runs the bench argv directly (no shell).
* `probe::EnvironmentProbe` — shells `git`/`rustc`, reads the hardware profile.
* `bench_output::BenchOutputSource` — harvests each engine's output tree, `mtime`-scoped.
* `storage::Storage` — `LocalStorage`/`AzureBlobStorage`, selected by `build_storage` into a
  `StorageFacade`; `MemoryStorage` fake. `put` is **write-once** (`AlreadyExists`);
  `put_overwrite`/`delete` are the escape hatches and arm the cache-invalidation marker.
* `config_writer::ConfigWriter` — `install`'s config-file writer (`create_new`).
* `git_history::GitHistory` — read-only commit topology (`resolve`, `default_branch`,
  `merge_base`, `first_parent`, `committer_time`, `is_dirty`). `first_parent` pairs each SHA
  with its committer date and subject; `FakeGitHistory` models a canned graph.

**When you add an IO edge, follow the same pattern.** Never call `SystemTime::now()` in
orchestration — time comes from the injected `tick::Clock` (`new_tokio` in production,
`new_frozen_at` in tests). Never insert real-time delays in tests.

## Compute fan-out

Do **not** spawn ad-hoc threads (`std::thread::scope`, `map_parallel`-style helpers) for
compute. Route every parallel pass through the injected `anyspawn::Spawner` that
`analyze_with` threads through: production injects the Tokio blocking pool, tests inject
`cargo_bench_history_core::testing::synchronous_spawner` (reached via core's
`private-test-util` dev-dependency feature), so the work stays runtime-agnostic and Miri-safe.
New per-series logic must be side-effect-free. Flow and rationale: [`docs/analyze.md`](docs/analyze.md).

## Selection lockstep (analyze / list / prune / examine)

`analyze`, `list`, `prune`, and `examine` share one **data-set-selection pipeline** in
`analyze/mod.rs` (`Selection` + `parsed_facets` + `facet_filtered_candidates` +
`resolve_history`/`select_dataset`), and all four live inside the `analyze` module tree
(`list.rs`, `prune.rs`, `examine.rs`, each `pub(crate) mod`; `bless`/`unbless` in `bless.rs`
reuse the same facet selection). **A selection parameter added to one must be added to all
four** unless genuinely inapplicable. The analysis-only flags (`--mode`,
`--include-improvements`, `--include-inactive`) are **not** part of the lockstep — only
`analyze` detects; `list`/`prune`/`examine` reuse the selection but never analyze. Each is
generic over the `GitHistory` + `Storage` ports so tests drive it with fakes + `block_on`.
Semantics and per-command behaviour: DESIGN §7–§8.

`analyze` needs a resolvable repository (topology at query time); `list discriminants` is the
one query view that does **not** (a pure index over storage keys). `examine` is the one
command that names a `--metric` (its input is an `analyze` finding, which prints the metric).

## CLI (clap derive)

The CLI uses **clap** derive (chosen over `argh` for `help_heading` grouping — the surface is
wide). Flags live in shared `#[command(flatten)]` arg-group structs so a group looks identical
everywhere and the selection lockstep holds automatically. Do not assume the other workspace
cargo tools share these grouped conventions. Full group/flag map: DESIGN §7.

## Verbose diagnostics (`report::Reporter`)

`--verbose` threads a `&dyn report::Reporter` through each pipeline. Emit notes **only**
through the guarded `ReporterExt` surface — `note_with(|| format!(…))` for one lazy line,
`if_enabled(|notes| …)` for a block; the unconditional `Sink` is sealed, so there is no
`enabled()`/`note()` guard to forget. Stage timings are a separate channel
(`ReporterExt::timing`). Production `StderrReporter` writes `[bench-history] …` to **stderr
only when verbose** (never stdout, so machine-readable output stays clean). The reporter is
`&dyn` (not `+ Sync`) so run futures stay `!Send` / Miri-driven. Notes must be **explanatory,
not conclusion-only** (see `docs/standalone-binaries.md`). Tests use the `#[cfg(test)]`
`RecordingReporter`.

## Storage & engines (essentials)

* Object bodies are **gzip** at the `Storage` boundary via `cargo_bench_history_core::codec`;
  callers hand/receive plain JSON. `MemoryStorage` deliberately stays **plaintext** (keeps the
  Miri `analyze` suite fast) — do not compress it.
* The backend is chosen at run time by `--local` / configured cloud (never a local path in the
  committed config); read commands add a `--cache` read-through mirror. `--machine-key` is
  CLI-only for the same reason (the config is committed). Details: DESIGN §4, §6.
* Four engines are parsed in `bench/` — deterministic (`callgrind`, `alloc_tracker`) vs noisy
  (`criterion`, `all_the_time`). `collect`/`backfill` invoke `cargo bench` once and harvest
  whatever ran; `--engine` is an `analyze` facet, not a collect flag. Details: DESIGN §1.

## Testing

**Miri.** Pure logic and fake-driven orchestration tests are plain `#[test]` +
`futures::executor::block_on` + a frozen clock (no runtime, no IO) so they run under Miri. Any
test touching the real filesystem/process/runtime/wall-clock or the network emulator must be
`#[tokio::test]` (or `#[test]`) **and** `#[cfg_attr(miri, ignore = "…")]` with a reason.

**Mock engine.** End-to-end tests launch the standalone `publish = false` **`mock_bench_engine`
package** (kept out of the shipped crate so `cargo install` places only the one real binary).
Tests resolve it via `mock_bench_engine::binary_path()`; every `just` recipe that runs the
suite under nextest pre-builds it once and passes `MOCK_BENCH_ENGINE` (avoids per-process
`cargo build` races). Its crate root needs
`#![cfg_attr(coverage_nightly, feature(coverage_attribute))]`. It writes per-engine fixture
output on demand via repeatable flags (`--summary`/`--criterion`/`--alloc-tracker`/
`--all-the-time`) and `--fail-if-exists` to simulate a failing commit.

**CWD & target root.** The real-adapter e2e test changes CWD, so it is `#[serial]`; on Windows
restore the original CWD **before** the `TempDir` drops. Each test pins its own tempdir target
root through the `run_with_overrides` entry (injected as `CARGO_TARGET_DIR` for the engine) —
**do not** reintroduce a `set_var("CARGO_TARGET_DIR", …)` guard. `tests/cbh_real_engine.rs` is
the one true e2e (real `cargo bench` + Criterion) and the only exercise of
`resolve_target_root`; keep it and its `resolve_target_root` guard test.

**Fixtures.** `tests/fixtures/**` are **real** committed engine output doubling as schema-drift
canaries — do not hand-edit to make a test pass; regenerate from a real run.

**Backfill coverage.** Planning/skip/overwrite/error logic is exhaustively covered by
fake-driven unit tests in `commands/backfill.rs`; keep the real-git integration drives a small
smoke subset (each spawns many git subprocesses and dominates mutation cost) — add new
coverage at the fake level.

**Mutation.** SDK-delegating Azure IO methods carry `#[cfg_attr(test, mutants::skip)]` (only
Azurite covers them); their pure logic stays under mutation. A test with no mutation signal
opts out with `#[cfg_attr(mutants, ignore = "<why>")]` — `just mutants` sets `--cfg mutants`
(registered in `unexpected_cfgs`).

**Azure.** The Azure backend is always compiled in (Entra-OAuth only). Network tests
**self-skip** when no emulator/account is reachable. Run them with `just test-azurite` (local
Azurite; sets `BENCH_HISTORY_REQUIRE_AZURITE=1`) or `just test-azure` (real account; sets
`ENABLE_AZURE`, reads the account from `constants.env`, needs `az login`). Auth/seam design:
DESIGN + `storage/azure.rs`.

## Stress harness

`cargo-bench-history-stress` (sibling package) is an on-demand scaling experiment for
`analyze` with **zero production-code coupling**: it writes objects in the exact storage key
layout + gzip body (via `core::codec`) and reads them back through the public
`run_with_overrides` entry — so if you change the on-disk key layout or the JSON report's
top-level fields, update its `seed.rs`/`report.rs` in lockstep. Run via
`just bench-history-stress[-azure]`; dataset shape is in its `README.md`.
