# cargo-bench-history

A Cargo subcommand that maintains a long-lived history of benchmark results and
analyzes it for trends that snapshot-only tooling cannot see — for example
"benchmark X has been getting incrementally slower over the past 12 months" or
"scenario Y regressed after commit Z, visible only in hindsight against noisy
data".

## How it works

Most benchmark tooling only reports the current run, or at best compares against
the previous local run. `cargo-bench-history` instead stores **every** run as an
immutable record (locally or, later, in cloud blob storage) and reconstructs
per-benchmark series ordered by an *effective* timestamp, so historical trends
become analyzable.

Results are partitioned only by what makes them fundamentally incomparable —
project, engine system, target triple, and (for hardware-dependent engines) a
machine key. Everything else (toolchain version, OS, commit, CI provider) is
recorded as metadata so its effect stays visible as a step in the timeline rather
than forking history.

The initial target engines are the ones this workspace uses: Criterion
(wall-clock) and Callgrind via Gungraun (simulated instruction counts).

## Commands

```text
cargo bench-history run [--engine NAME] [--timestamp RFC3339]
                        [--target-triple TRIPLE] [--no-store]
                        [--config PATH] -- <args forwarded to each engine>
cargo bench-history install [--config PATH]
cargo bench-history analyze [--since DATE] [--system SYSTEM]
                            [--format text|json|markdown]
                            [--fail-on-regression] [--config PATH]
```

* `run` executes each configured engine, harvests its output, and stores the
  result set. `--timestamp` overrides the effective time when backfilling history
  for an old commit; everything after `--` is forwarded verbatim to each engine
  command (use `--engine` to target a single engine).
* `install` generates a starter `.cargo/bench_history.toml` if absent.
* `analyze` downloads a partition and reports notable patterns; `--fail-on-regression`
  enables CI gating.

## Status

This crate is at **Phase 0**: the data model, configuration, comparability and
storage foundations are in place and the subcommands parse, but the command
handlers are stubs that report the command as recognized-but-not-yet-implemented.
See `DESIGN.md` for the full design and iteration plan.
