# Agent notes for cargo-bench-history

This is an async-by-default Tokio application. The architecture keeps all pure
logic (parsing, mapping, comparability, key derivation, message formatting)
synchronous and pushes async only to the IO edges, each modelled as a small
"port" trait with a real Tokio adapter and an in-`#[cfg(test)]` in-memory fake.

## Ports and fakes

The orchestrator (`commands::run::execute_run`) is generic over its ports and is
driven in tests by fakes, never by real IO:

* `process::BenchRunner` — real `TokioBenchRunner` runs the engine command's
  argv directly (no shell — the configured command is tokenized with POSIX
  shell-word rules and spawned, so forwarded arguments are passed verbatim);
  fake `FakeRunner` records the argv and reports a canned exit status.
* `probe::EnvironmentProbe` — real `SystemProbe` (shells `git`/`rustc`); fake
  `FakeProbe` returns canned `GitSnapshot`/`RustcInfo`.
* `bench_output::BenchOutputSource` — real `FsBenchOutputSource` (walks
  `target/gungraun/**/summary.json`); fake `FakeOutput` returns in-memory
  `RawSummary` values.
* `storage::Storage` — real `LocalStorage` and (behind the `azure` feature)
  `AzureBlobStorage`, selected at runtime by the `StorageFacade` enum that
  `build_storage` returns; fake `MemoryStorage`.
* `config_writer::ConfigWriter` — real `TokioConfigWriter` (creates parent dirs,
  `create_new` so an existing file is never clobbered); fake `MemoryConfigWriter`.
  Used by `commands::install::execute_install`. Its real adapter's IO error paths
  are covered by `#[cfg_attr(miri, ignore)]` real-filesystem tests in
  `config_writer::real_writer_tests` (blocked parent, interior-NUL open error).

When you add a new IO edge, follow the same pattern: a port trait with an
`impl Future` return (RPITIT, no `async_trait`), a real adapter, and a fake.

## The `analyze` command

`analyze::execute` builds the real `LocalStorage` and delegates to
`analyze::analyze_with`, which is generic over the `Storage` port so tests drive
it with `MemoryStorage` + `futures::executor::block_on` (Miri-safe, no Tokio).
Everything below the storage load is pure and synchronous:

* `analyze::series` reconstructs one series per `(location, benchmark, metric)`
  ordered by effective time (then object key as a deterministic tie-break).
  `--since` filters at the object level — whole runs before the cutoff are
  dropped — so the reported run count and every series share one window.
* `analyze::findings` is the rolling-baseline regression detector (median
  baseline over a bounded window, MAD-aware threshold, severity tiers). Keep it
  deterministic and cover boundaries with named value-asserting tests, not
  threshold guards.
* `analyze::report` renders text/json/markdown. Rendering is infallible: the
  report is plain structs of finite numbers, so the JSON path uses `.expect`
  rather than threading a serialization error nobody can trigger.

## Time is injected — never read the wall clock or sleep

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
so the mock writes where the harvester reads. Because the engine runs without a
shell, the config `command` embeds the binary path POSIX-single-quoted (so it
survives shell-word tokenization as one argument) and TOML-escaped (so Windows
backslashes survive the TOML string). Like every other crate root that uses
`#[cfg_attr(coverage_nightly, coverage(off))]`, the mock engine must declare
`#![cfg_attr(coverage_nightly, feature(coverage_attribute))]` at its top, or the
coverage job fails to compile it (E0658).

## End-to-end tests and the current directory

The real-adapter end-to-end test changes the process current directory into a
tempdir, so it must be `#[serial]` (from `serial_test`). On Windows a `TempDir`
cannot be dropped while it is the current directory, so restore the original CWD
**before** running assertions and before the tempdir is dropped. Prefer setting
`[project] id` in the test config rather than relying on the CWD basename.

Each test also points the harvest at its own tempdir `target/` for the duration
of `run`, by passing an explicit target root through the `run_with_target_root`
entry. `run` injects that resolved root into every engine's environment as
`CARGO_TARGET_DIR`, so the engine writes exactly where the harvest scans. This
matters under the coverage job, which runs `cargo llvm-cov nextest` and redirects
every build and bin into one shared `target/llvm-cov-target`: that ambient
`CARGO_TARGET_DIR` would otherwise make the mock engine (which honors it) write
its summaries into the shared directory while the overridden harvester looked in
the tempdir, harvesting nothing. Pinning both ends to the same root keeps each
test hermetic **without mutating the process environment** — do not reintroduce a
`set_var("CARGO_TARGET_DIR", …)` guard. The production `run` entry passes `None`,
resolves the root from the environment as usual, and injects that same value so
the real engine and harvest never diverge either.

## Fixture-golden canaries

`tests/fixtures/callgrind/*.summary.json` are **real** Gungraun output committed
verbatim. They are schema-drift canaries used by the parser tests, the
orchestration tests, and the integration test. Do not hand-edit them to make a
test pass — regenerate from `just bench-cg` if the upstream schema genuinely
changes, and update the parser/tests to match.

## WSLENV (do not add magic WSL detection)

When injecting engine environment variables, `TokioBenchRunner` unconditionally
appends their names to `WSLENV` with the `/u` up-flag (`bench::merge_wslenv`).
This is inert outside WSL and makes the injected env cross the Windows→Linux
boundary regardless of how the command launches, so we never try to detect
whether a command crosses into WSL. The golden rule still holds as guidance: run
the tool in the same OS as the benches.

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
