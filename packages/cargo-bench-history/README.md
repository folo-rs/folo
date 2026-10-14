# cargo-bench-history

Keep benchmark history across commits and find regressions that a comparison with the
previous run can miss: a step near a change, or slow drift across many changes.
On pull requests, compare the branch tip with the base's observed range and report both
regressions and improvements.

The tool runs your existing **Criterion, Gungraun/Callgrind, alloc_tracker or
all_the_time** benchmarks, stores their measurements on local disk or in Azure Blob
storage, and reconstructs comparable series in Git first-parent order. Measurements
from different engines, targets or hardware partitions do not mix.

**Findings are advisory.** Reports explain the units, direction, comparison evidence
and judged coverage. An empty report is not automatically an all-clear, and an
execution failure is not a benchmark verdict.

## See a report first

The [illustrated report walkthrough](https://folo-rs.github.io/folo/cargo-bench-history/appendix/reporting.html#worked-report-gallery)
shows production-generated step, drift, clean and inconclusive examples, a
PR-comment-style summary and a workflow result card. Download the text, Markdown
or JSON examples without running benchmarks or provisioning storage. They are
worked examples, not measurements of your repository.

## Try one benchmark locally

Install a prebuilt binary with
[`cargo binstall cargo-bench-history`](https://github.com/cargo-bins/cargo-binstall),
or build from source with `cargo install cargo-bench-history`.

The [local quickstart](https://folo-rs.github.io/folo/cargo-bench-history/getting-started.html)
checks your engine and target, separates local storage from build output, and collects
one chosen target before analyzing it. First success is a stored, inspectable measurement,
not a promised finding. A new history usually needs more comparable commits.

| Your next task | Guide |
| --- | --- |
| Understand a finding or missing evidence | [Read a report](https://folo-rs.github.io/folo/cargo-bench-history/appendix/reporting.html) |
| Add push, PR and bounded backfill workflows | [Connect GitHub](https://folo-rs.github.io/folo/cargo-bench-history/github-automation.html) |
| Reproduce, fix or accept a scoped tradeoff | [Investigate and accept](https://folo-rs.github.io/folo/cargo-bench-history/appendix/insights.html) |
| Bootstrap a baseline and manage cost, retention and upgrades | [Operate benchmark history](https://folo-rs.github.io/folo/cargo-bench-history/operations.html) |
| Check engine output and benchmark identity | [Benchmark engines](https://folo-rs.github.io/folo/cargo-bench-history/concepts/engines.html) |

## Shared history and automation

The [reusable GitHub workflows](https://github.com/folo-rs/cargo-bench-history-action)
collect history on base-branch pushes, compare affected PR benchmarks and backfill
missing commits. Workflow cards separate execution, analysis coverage, platform
completeness and publication. Full evidence remains in report artifacts even when
issue/comment publication is disabled; disabling publication does not disable storage.

The [Azure setup guide](https://folo-rs.github.io/folo/cargo-bench-history/commands/setup-azure.html)
provides a reviewable deployment bundle and short-lived federated authentication.
The [ordered GitHub guide](https://folo-rs.github.io/folo/cargo-bench-history/github-automation.html)
connects storage, repository variables and caller workflows without stored credentials.

Use [ignore prefixes](https://folo-rs.github.io/folo/cargo-bench-history/commands/analyze.html#ignoring-benchmarks)
to keep experimental measurements available but outside analysis. Use
[scoped blessings](https://folo-rs.github.io/folo/cargo-bench-history/commands/bless.html)
to accept an intentional change and start a new baseline. These are distinct policies,
not ways to turn missing evidence into a clean report.

The [user guide](https://folo-rs.github.io/folo/cargo-bench-history/) includes the command
reference and statistical pipeline. [API documentation](https://docs.rs/cargo-bench-history/)
covers the library surface.

Part of the [Folo project](https://github.com/folo-rs/folo).
