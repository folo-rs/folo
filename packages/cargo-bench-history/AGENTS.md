# Agent notes for cargo-bench-history

This is an async-by-default Tokio application. The architecture keeps all pure
logic (parsing, mapping, comparability, key derivation, message formatting)
synchronous and pushes async only to the IO edges, each modelled as a small
"port" trait with a real Tokio adapter and an in-`#[cfg(test)]` in-memory fake.

## Ports and fakes

The orchestrator (`commands::run::execute_run`) is generic over its ports and is
driven in tests by fakes, never by real IO:

* `process::BenchRunner` — real `TokioBenchRunner` (shells the engine command);
  fake `FakeRunner` records the invocation and reports a canned exit status.
* `probe::EnvironmentProbe` — real `SystemProbe` (shells `git`/`rustc`); fake
  `FakeProbe` returns canned `GitSnapshot`/`RustcInfo`.
* `bench_output::BenchOutputSource` — real `FsBenchOutputSource` (walks
  `target/gungraun/**/summary.json`); fake `FakeOutput` returns in-memory
  `RawSummary` values.
* `storage::Storage` — real `LocalStorage`; fake `MemoryStorage`.

When you add a new IO edge, follow the same pattern: a port trait with an
`impl Future` return (RPITIT, no `async_trait`), a real adapter, and a fake.

## Time is injected — never read the wall clock or sleep

All time comes from an injected `tick::Clock`. Production uses
`Clock::new_tokio()`; tests use `Clock::new_frozen_at(...)` for deterministic
keys and timestamps. Do not call `SystemTime::now()` in orchestration code and
never insert real-time delays in tests.

## Miri strategy

* Pure logic and fake-driven orchestration tests are plain `#[test]` using
  `futures::executor::block_on` plus a frozen clock — no Tokio runtime, no IO —
  so they run under Miri.
* Any test that touches the real filesystem, spawns a process, or starts a Tokio
  runtime must be `#[tokio::test]` (or `#[test]` for `std::fs`) **and**
  `#[cfg_attr(miri, ignore)]` with a comment saying why. These are the 13 tests
  Miri skips.

## End-to-end tests and the current directory

The real-adapter end-to-end test changes the process current directory into a
tempdir, so it must be `#[serial]` (from `serial_test`). On Windows a `TempDir`
cannot be dropped while it is the current directory, so restore the original CWD
**before** running assertions and before the tempdir is dropped. Prefer setting
`[project] id` in the test config rather than relying on the CWD basename.

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
