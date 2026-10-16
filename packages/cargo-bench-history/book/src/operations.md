# Operate benchmark history

Keep collection comparable, make missing evidence visible, and bound the work needed
to maintain it. The commands below are operating choices, not detection guarantees.
For an individual result, start with [Read a report](appendix/reporting.md) or the
[troubleshooting table](appendix/insights.md#symptom-to-next-action).

## Bootstrap and check a baseline

A baseline needs **distinct commits with comparable measurements**, not repeated
executions of one commit. The project, engine, target and machine must match.
Measurements on another machine key do not fill holes in this partition.

<figure class="chart" tabindex="0" role="group" aria-label="Chart; scroll horizontally on small screens">
{{#include appendix/generated/collection-occupancy.svg}}
</figure>

Each row is one partition, not another view of a shared pool of measurements.
A workflow can collect on every expected platform and still leave sparse history
inside a particular machine row. Conversely, the available series may be fully
judged while a platform job is missing.

Start with `list discriminants`, then use the report's explicit discriminant
selectors with `list runs` and `examine`. Query defaults are local-machine defaults;
they are not a promise to select a CI report's hardware. Read the per-reason census
in `report.json` or full Markdown before deciding whether more collection will help.

Choose a recent first-parent range whose benchmarks still build. For example, if
`HEAD~3` through `HEAD` are the intended base-branch commits:

```console
cargo bench-history backfill --local=.bench-history HEAD~3 HEAD --max-commits 1 --package textproc --bench parse --best-of 1
```

Here the package and bench names stand for your chosen target. Review the actual
commit range first; on a PR branch, use explicit **base** refs instead of its `HEAD`.
Use an absolute local storage path when invoking from different directories.
Backfill uses an isolated historical worktree and manages its build output separately
from the local quickstart's target directory.

The example attempts at most one missing commit, not one benchmark iteration or one
second of work. Repeat the same frozen range and protocol in the same partition to
fill the next gap. Check the final stored, skipped, empty, failed and deferred counts.
See [bounded passes](commands/backfill.md#bounded-passes) for the counting rules and
machine-readable completion output.

Readiness depends on the analysis mode, measured series, gaps, noise, accepted baseline
boundaries and the suite being judged. A fixed lookback duration or passing a detector's
minimum point count does **not** guarantee a finding. After a recent blessing, usable
base evidence may need to accumulate again.

## Choose the right operation

| Intent | Operation | What remains distinct |
| --- | --- | --- |
| Execute a benchmark again without replacing clean stored evidence | `collect --skip-existing` | Execution still runs. A newly captured collection snapshot can differ from the preserved shared object. |
| Replace an incorrect clean measurement | `collect --overwrite` | Explicitly changes stored evidence and invalidates read caches; it is not an acceptance decision. |
| Fill missing commits | `backfill` with `--max-commits` | Recorded commits are skipped before execution in the current partition. Overwrite starts at the newest commit each time, rather than resuming. |
| Test collection without writing history | `collect --no-store` | It still benchmarks. This differs from disabling GitHub issue/comment publication. |
| Inspect observations without rerunning detection | `examine` or `list` | These preserve stored data and do not bless a finding. |
| Keep experiments stored but outside analysis | `[ignore].benchmarks` prefixes | Analysis discloses exclusions. Collection is unchanged; removal makes the stored series eligible again. |
| Accept an intentional change for selected benchmarks | `bless` | Sets an analysis baseline boundary; earlier observations remain stored. Scope defaults to all engines, targets and machines. |
| Revoke acceptance | `unbless` | Removes complete blessing records at the chosen context, not one point or one prefix within a record. |

[`collect`](commands/collect.md), [`backfill`](commands/backfill.md),
[ignore prefixes](commands/analyze.md#ignoring-benchmarks), and
[`bless / unbless`](commands/bless.md) own the full command contracts.
Do not use overwrite, ignore or blessing merely to make an unexplained finding disappear.

## Bound cost and retain the right evidence

Start with a small benchmark target and a reviewed range. Benchmark execution dominates
replay cost; compilation, repetitions, native tools and cloud transfers also matter.
`--max-commits` bounds attempts after skip checks, not wall-clock time. A timeout or
cancelled runner is not successful bounded completion. Read completion evidence where
available; missing evidence means unknown counts, not zeros.

Keep the collection scope consistent for a clean commit. Per-engine clean results
are stored as whole objects, not merged from disjoint package runs. Backfill's skip
scan treats a commit recorded by any engine in the selected partition as recorded;
inspect partial engine writes before choosing an explicit overwrite.

Shared **storage** is the durable observation history. Workflow **artifacts** retain
reports, diagnostics and, where produced, exact-current collection snapshots or
backfill completion evidence. Artifact expiry does not prune the store, and keeping
a JSON report does not preserve every underlying observation. Choose artifact retention
to cover your review and investigation cycle, and keep snapshots when exact rerun
evidence matters.

Use [`prune --dry-run`](commands/prune.md) to review storage deletion before executing
it. Removing observations changes later analysis. Do not delete the local history
directory as part of build cleanup. The [read-through cache](storage.md#read-through-cache-cloud-backend-ci)
is disposable; it is not another authoritative history.

## Change the measurement protocol deliberately

Treat target selection, input data, Cargo features, build profile, compiler flags,
engine settings and `--best-of` as one measurement protocol. Keep it aligned across
history, PR and backfill callers. Required features can make a target disappear from
aggregate Cargo builds; explicit target checks help distinguish this from an analysis
problem.

Toolchain versions and repetition counts are metadata, not partition keys. A compiler
upgrade or a different minimum-of-repetitions policy can appear as a measured step.
Record the intended protocol change in the repository, inspect correlated metrics
and diffs, and accept only the justified scope through
[the investigation workflow](appendix/insights.md#accept-an-intentional-change).
Do not assume every suite-wide move has an infrastructure cause.

Backfill uses each historical checkout's selected Rust toolchain, while caller
features and compiler flags express today's replay request. It does not recreate every
historical CI environment. See [backfill configuration](commands/backfill.md#toolchain-and-measurement-configuration)
and [measurement stability](concepts/stability.md).

## Upgrade tools and automation together

When adopting the v3 reusable workflows, add `actions: read` to existing backfill
callers as shown in [GitHub onboarding](github-automation.md#7-bootstrap-comparable-base-history).
The workflow needs job-attempt evidence for trustworthy rerun reconciliation;
changing only the `uses:` ref is not the complete caller upgrade.

Review the selected [action release](https://github.com/folo-rs/cargo-bench-history-action/releases)
and its documented tool pins and installation choices. Use the owning action
documentation for supported runner/target pairs and action inputs, rather than
assuming that a locally installed CLI and an arbitrary workflow ref form a supported
pair.

Keep history, PR and backfill callers on the same intended release and protocol.
Check native engine artifacts, one collected result, the generated report and its
coverage before expanding an upgrade across the history. Features such as project-level
blessings require readers as well as writers that understand their stored format.
Changing the action ref does not re-measure old commits or retroactively rewrite stored
observations.
