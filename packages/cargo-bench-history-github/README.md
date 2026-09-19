# cargo-bench-history-github

Unsupported GitHub automation companion for
[`cargo-bench-history`](https://folo-rs.github.io/folo/cargo-bench-history/).
The reusable action pins a tested companion release; neither this binary nor its
library has a stable API.

## Installation

`cargo binstall cargo-bench-history-github` installs a prebuilt binary on supported
targets, with a transparent source-build fallback otherwise.

## Root action execution

The unsupported `action --inputs-file PATH --github-output PATH --temp-dir PATH [--tool PATH]`
entry point executes the root action after its bootstrap installs the required binaries.
It validates command-specific inputs, resolves the configured project namespace, drives
collection or analysis, and delegates publication to the same lifecycle commands below.
The measured working directory is independent of the installation source checkout.
See the [bootstrap-facing contract](docs/action.md) for inputs, paths and outputs.

## Publication

The companion embeds the tool-rendered summary and validates the accompanying JSON
report and collection-platform evidence. It publishes through
`publish-comment-{findings,clean,preflight,no-data,failed}` and
`publish-issue-{findings,clean,preflight,no-data,failed}`.

Any notable findings select findings publication, even when coverage is partial.
Clean publication requires a fully judged, nonempty analysis and every intended platform.
No-data explains why a successful analysis without findings cannot establish a complete verdict,
preserving the judged portion and explaining missing coverage. An explicit `--empty-scope` form
covers runs with no benchmarkable packages; execution failures are not analysis verdicts.

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

The HTTP adapter prefers a nonblank `GITHUB_TOKEN` from workflow environments and otherwise
accepts `GH_TOKEN` from environments prepared for GitHub CLI use. It does not read the CLI's
credential store or change tokens after an authentication failure. Repository inputs
fall back to `GITHUB_REPOSITORY`. Matrix setup and report inspection need neither a
repository nor credentials; receipt creation also runs without credentials.
