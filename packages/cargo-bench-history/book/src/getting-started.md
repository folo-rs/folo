# Try locally

<a id="getting-started"></a>

First success means **one known benchmark ran, its measurements were stored, and you
can inspect the resulting report**. It does not require a regression or an existing
history. This path uses local disk only; it needs no Azure account or GitHub workflow.

## See a result without running benchmarks

Open the [worked report gallery](appendix/reporting.md#worked-report-gallery) for
a known step, drift and inconclusive result. The maintained examples come from the
production detectors and renderers. Their text, Markdown and JSON downloads need no
checkout, historical replay or import command. They are not executed user measurements.

## Check the prerequisites

[Install the tool](installation.md), then run these commands from your project's
Git checkout:

```console
cargo bench-history --help
git status --short
cargo metadata --no-deps --format-version 1
```

Use the metadata's `packages[].targets` to choose **one** target with `kind: ["bench"]`
and its owning package. Check `required-features` and enable the same features in every
command below. A compile-only check is not a measurement: the target must actually
execute a [supported engine](concepts/engines.md#check-native-output).

The examples use package `textproc` and bench target `parse`. Substitute your real
package and target; these names do not create a benchmark. If the repository has no
compatible benchmark, use the gallery now and add one through the engine's own guide
before collecting.

## Choose separate storage and build directories

Use `.bench-history` for the local history, `.bench-target` for Cargo and engine
output, and `.bench-reports` for reports. Add all of them to your checkout's local
`.git/info/exclude` before proceeding. For a linked worktree, `git rev-parse
--git-path info/exclude` locates that file. Keeping generated files ignored avoids
turning a clean measurement into a dirty snapshot. Do not place your only copy of
history inside a Cargo target directory: `cargo clean` removes build output.

Set Cargo's output directory explicitly rather than inheriting another checkout's
`CARGO_TARGET_DIR`. In PowerShell:

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true

$env:CARGO_TARGET_DIR = Join-Path (Get-Location) ".bench-target"
New-Item -ItemType Directory -Force .bench-reports | Out-Null
```

In a POSIX shell, use `export CARGO_TARGET_DIR="$PWD/.bench-target"` and
`mkdir -p .bench-reports` instead. Run the remaining commands in that same shell.
Relative storage paths here are relative to the
invocation directory. If using directories outside the checkout, pass absolute paths
consistently. A local path does not belong in the committed configuration.

## Collect a small, deliberate scope

Optionally run `cargo bench-history install` once to write a commented configuration;
it preserves an existing file. Give `[project].id` a stable name if different checkout
directory names should share history. Commit the configuration and benchmark changes
before collecting a clean baseline. Uncommitted work is useful for local branch
inspection, but does not become clean base history.

```console
cargo bench-history collect --local=.bench-history --package textproc --bench parse --best-of 1 --verbose
```

This runs the chosen target once, not the entire workspace or a historical range.
It bounds scope and repetitions, not elapsed time; the engine controls its own
measurement duration. Keep this repetition count for later comparisons.

**Checkpoint:** the diagnostic stream names the effective target and machine key.
Check which engine emitted measurements and whether anything was stored. An empty
harvest is not a successful baseline. Inspect the
[native artifact checklist](concepts/engines.md#check-native-output) before widening
scope or replaying commits. Do not use engine smoke/test mode as a substitute for
measurement.

Ordinary collection rejects an existing clean result. For a deliberate rerun,
`--skip-existing` still executes the benchmark but preserves stored data;
`--overwrite` replaces it. See [rerun choices](operations.md#choose-the-right-operation)
before replacing evidence.

## Inspect the first result

Run:

```console
cargo bench-history list discriminants --local=.bench-history
cargo bench-history list runs --local=.bench-history
cargo bench-history analyze --local=.bench-history --markdown .bench-reports/report.md --json .bench-reports/report.json --markdown-summary .bench-reports/summary.md
```

**Checkpoint:** `list discriminants` shows the engines, targets and machine keys
actually stored. `list runs` shows the selected runs. The analyzer prints its
selection to stderr, a readable report to stdout, and writes the requested files.
Open `report.md`; JSON's `outcome` and `census` carry the machine-readable result.
If the repository has no discoverable default branch, pass its real base ref with
`--base` rather than assuming a branch name.

A new store normally has **insufficient baseline evidence**. Some selections may
have nothing in scope instead; read the diagnostic and coverage reasons rather
than treating silence as clean. Dirty working-tree snapshots and repeated runs of
one commit do not create a history of distinct committed changes.

## Grow a baseline only when you need it

Keep collecting as commits reach the base branch, or choose a small reviewed
first-parent range for [bounded backfill](commands/backfill.md#bounded-passes).
Check that its historical targets build with your selected features and protocol.
The [baseline-readiness guide](operations.md#bootstrap-and-check-a-baseline)
explains how to inspect comparable history; neither a calendar span nor a minimum
point count guarantees a finding.

Next, [read a report](appendix/reporting.md), then [connect GitHub](github-automation.md)
when you want a shared history.
