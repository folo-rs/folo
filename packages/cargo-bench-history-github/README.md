# cargo-bench-history-github

Unsupported GitHub automation companion for
[`cargo-bench-history`](https://folo-rs.github.io/folo/cargo-bench-history/).
The reusable action pins a tested companion release; neither this binary nor its
library has a stable API.

## Installation

`cargo binstall cargo-bench-history-github` installs a prebuilt binary on supported
targets, with a transparent source-build fallback otherwise.

## Publication

The companion embeds the tool-rendered summary and validates the accompanying JSON
report and collection-platform evidence. It publishes through
`publish-comment-{findings,clean,preflight,no-data,failed}` and
`publish-issue-{findings,clean,preflight,no-data,failed}`.

Findings remain findings when coverage is partial. Clean publication requires a
fully judged, nonempty analysis and every intended platform. No-data explains why a
successful analysis cannot establish a complete verdict, preserving useful partial
results. An explicit `--empty-scope` form covers runs with no benchmarkable packages;
execution failures are not analysis verdicts.

Only findings create rolling issues. Clean leaves an existing issue open. No-data
and failure annotations preserve its previous report. Comment preflight and failure
publication track the workflow run, attempt and frozen head so older terminal steps
cannot retire newer placeholders.

Rolling issues are found by their project-qualified title; its UTC date records the
last body update, not measurement freshness. `alert --run-id N --run-url URL` files
one issue for that workflow run. Existing alerts, including human-closed alerts,
are left unchanged.

Use `--help` on each command for its arguments. Common `--repository`, `--instance`
and `--verbose` options precede the subcommand. The instance is internal data derived
from the configured project ID, not an additional action setting.

## Workflow evidence

`workflow-matrix`, `collection-receipt`, `prepare-analysis` and `inspect-report`
provide matrix, collection-attempt and report evidence for workflow orchestration.
Inspection emits `publication-state=findings|clean|no-data` from the same validation
used by publication; `can-clear` applies only to history issues.

The HTTP adapter uses `GITHUB_TOKEN`, falling back to `GH_TOKEN`. Repository inputs
fall back to `GITHUB_REPOSITORY`. Matrix setup and report inspection need neither a
repository nor credentials; receipt creation also runs without credentials.
