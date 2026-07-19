# cargo-bench-history

A Cargo subcommand that keeps a long-lived history of your benchmark results and
analyzes it for regressions and drift that snapshot-only tooling cannot see — a
scenario that got 12% slower over six months, or a regression at commit Z that is
only obvious in hindsight against noisy data.

## What it does for you

Most benchmark tooling reports only the current run, or at best diffs it against
the previous one. `cargo-bench-history` stores **every** run as an immutable
record and reconstructs each benchmark's series in git first-parent commit order,
so it can tell a real trend apart from run-to-run noise and point at the commit
that moved it.

Analyzing that history is what turns it into a verdict. On a pull request it runs
in **branch mode** — judging the branch tip against the base level — and produces a
report like this: one regression and one improvement, each charted against the
baseline it is compared to (the flat run on the left) so the tip's step is obvious:

```text
Analyzed project folo (branch mode)
  commit: 4f2a1c9
  runs: 218 (9c1d0ab → 4f2a1c9)  regressions: 1  improvements: 1

callgrind/x86_64-unknown-linux-gnu/synthetic
  runs: 218  regressions: 1  improvements: 1
  filter: --engine callgrind --target-triple x86_64-unknown-linux-gnu --machine-key synthetic

many_cpus/hardware_info/query
  +13.00% instruction_count (99% confidence)
    regression via change point · 100 → 113 · @ 4f2a1c9
 113 ┤                               ╭───────────────
 110 ┤                             ╭─╯
 106 ┤                           ╭─╯
 103 ┤                         ╭─╯
 100 ┼─────────────────────────╯

events_once/emit/one_subscriber
  -15.00% instruction_count (98% confidence)
    improvement via change point · 50 → 42.5 · @ 4f2a1c9
 50.44 ┼─────────────────────────╮
 48.45 ┤                         ╰─╮
 46.47 ┤                           ╰─╮
 44.48 ┤                             ╰──────╮
 42.50 ┤                                    ╰──────────
```

What you get out of it:

- **High signal-to-noise.** No benchmark engine is deterministic, so every metric
  is treated as noisy and gated hard — the tool would rather stay quiet than cry
  wolf. Each finding carries its confidence and the commit it is attributed to.
- **Regressions *and* improvements**, each judged at the branch tip and charted
  against its baseline so you can see the size and shape of the change at a glance.
- **A machine-readable verdict.** The same analysis emits JSON for automation;
  findings are advisory and never change the exit code, so a regression never
  breaks a build on its own — your automation decides what to do with the signal.

## Integrating it into your project

`cargo-bench-history` needs somewhere to keep history that outlives a single run:

- **Local disk** — pass `--local=<path>` (or a bare `--local` to read the path
  from `CARGO_BENCH_HISTORY_STORAGE`). Ideal for trying it out and for one
  developer's machine.
- **Azure Blob storage** — configure a container in `.cargo/bench_history.toml`
  and omit `--local`. This is the shared backend a team and CI read and append to.
  A local path is machine-specific, so it is never written into the shared config.

In CI today, integration is hand-crafted per project: a GitHub Actions workflow
runs `collect` on the base branch to grow the history, and runs `analyze` on each
pull request to judge the branch tip against the base, posting the result as a
**Performance impact** comment. (Reusable actions to make this turnkey are on the
roadmap; for now the workflow is written by hand.)

## Trying it locally

Install with [`cargo binstall cargo-bench-history`](https://github.com/cargo-bins/cargo-binstall)
to fetch a prebuilt binary on supported targets (building from source elsewhere),
or `cargo install cargo-bench-history` to always build from source. It then runs as
`cargo bench-history`.

`cargo-bench-history` records whatever your existing benchmarks emit — it does not
create them for you, so your project needs at least one benchmark a supported engine
can harvest (most commonly Criterion benches under `benches/`).

To see analysis you first need a history to analyze — a single run has nothing to
compare against. Grow one by backfilling past commits, then analyze the branch
you are working on:

```text
# Write a starter .cargo/bench_history.toml (documents the optional cloud backend).
cargo bench-history install

# Bench a range of past commits so analysis has a trend to work with. Runs in an
# isolated worktree and is resumable, so you can stop and re-run it.
cargo bench-history backfill --local=./bench-history <from-commit> <to-commit>

# Bench the current commit and append it to the history. On a noisy machine, run
# the suite a few times and keep the best (minimum) value per metric.
cargo bench-history collect --local=./bench-history --best-of 3

# Judge your branch's impact. On a feature branch this auto-selects branch mode,
# which compares the tip against the base and reports both regressions and gains.
cargo bench-history analyze --local=./bench-history

# Drill into the raw per-commit points behind a finding to correlate a change with
# the commit that caused it (both --benchmark and --metric are required).
cargo bench-history examine --local=./bench-history \
    --benchmark many_cpus/hardware_info/query --metric instruction_count
```

## Further reading

The [user guide](https://folo-rs.github.io/folo/cargo-bench-history/) walks through
every command, the storage backends and the analysis model in depth.

API details are in the [package documentation](https://docs.rs/cargo-bench-history/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides
mechanisms for high-performance hardware-aware programming in Rust.
