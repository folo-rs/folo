# Design

`cargo-bench-history-github` is the unsupported GitHub automation companion for
`cargo-bench-history`. It owns the GitHub-specific envelope and lifecycle around reports while
the main tool remains independent of GitHub.

The complete action design lives in
[`../../cargo-bench-history/docs/reusable-action.md`](../../cargo-bench-history/docs/reusable-action.md).
This package implements its GitHub lifecycle and workflow-evidence responsibilities.
It deliberately has no stable API
or command-line contract; the separately versioned action pins a tested companion version.

A **project** is one configured benchmark history. Its **project namespace** is the canonical
storage identity resolved by the core tool from the project configuration or directory fallback.
An **action instance** is that project's automation within a GitHub repository. Its internal
`instance` value carries the project namespace so reports, collection jobs and receipts for
different projects do not collide. It is not a separate consumer-selected identity.

## Post-install action execution

The action's PowerShell bootstrap selects the installation method and Folo source checkout,
installs the required binaries, and invokes the companion. The companion owns post-installation
execution: it validates command-specific string inputs before benchmark, storage or publication
work and drives the selected command. The measured working directory independently selects the
checkout and configuration; the project namespace uses the core tool's canonical storage identity.

Collection and backfill preserve the core tool's scope, feature, repetition and write-mode
choices. Analysis validates full Git history, resolves the context commit and uses only the
actual supplied machine keys. History analysis selects that commit as both context and base;
PR analysis accepts the caller's base or the core default and requires a branch-mode report.
Platform coverage is explicit workflow evidence, never inferred from deduplicated fingerprints.

Report artifacts occupy an invocation-owned temporary directory outside the checkout and
remain available for the job's artifact upload. Long-running tool output streams to the job
log. Only dedicated machine-key and Git responses are captured. Successful outputs are
appended only after the selected work and its evidence checks succeed.

Fork-origin PR events, including `pull_request_target`, skip benchmark and publication work
with a diagnostic and explicit skip outputs. Fork benchmarking is blocked pending supported,
secure federated access to the target branch's benchmark history. The repository's
`pull_request` OIDC subject does not itself distinguish a fork head from a same-repository head.
The companion does not initialize GitHub authentication for offline collection, backfill or
analysis. Child processes inherit the caller's environment for
builds and benchmarks. Publication reuses the existing report evidence and lifecycle policy;
missing execution identity is an error, not permission to invent a workflow run or verdict.

The unsupported bootstrap-facing contract is documented in [action execution](action.md).

## Responsibilities

The companion:

* publishes and updates the rolling regression issue and pull-request comment;
* marks an existing report stale while a new benchmark run is in flight;
* replaces a recovered regression issue with an all-clear state while leaving it open;
* files one-off workflow-failure alerts without automatically resolving them;
* retires pull-request placeholders after failure or when nothing benchmarkable changed;
* binds successful collection to actual workflow job attempts and measured machine keys; and
* reconciles an ambiguous create against the intended identity and content rather than retrying
  it blindly.

It embeds the Markdown summary rendered by `cargo-bench-history` verbatim. It never interprets
findings or re-derives analysis vocabulary.

## Standard reporting

Reports use a shared message catalogue, including advisory wording and the public
[cargo-bench-history guide](https://folo-rs.github.io/folo/cargo-bench-history/).
Regression issues use `Benchmark history findings for <project> (updated YYYY-MM-DD)`.
The project-qualified prefix identifies the rolling issue; the suffix is the UTC date of
the companion's last body update, not the last measurement. For example, repeating an identical
publication for the same run, attempt, commit and report artifacts does not change the existing
body. That request makes no issue update, so its title date remains unchanged even if the retry
occurs on another day.
Workflow-failure issues use `Benchmark history workflow failed for <project> (run <run-id>)`.
The caller supplies report artifacts and evidence, not titles, introductions, documentation
links or comment identities.

Regression issues remain open after all-clear so their rolling history is retained. Failure
alerts describe individual workflow runs and stay unchanged when later runs succeed.
Empty pull-request scope always creates or updates an explanatory note rather than deleting
the rolling comment.

A **staleness warning** identifies results for a head other than the head being reported.
It includes a commit distance when GitHub can verify the forward relationship. Otherwise the
**unknown-distance warning** says the results are out of date but the distance is unavailable;
it does not guess a count for unrelated history or an unavailable comparison. This differs from
the freshness-unverified warning used when the live PR head itself cannot be read.

## Publication states

Commands use `publish-<sink>-<state>`, with `comment` or `issue` as the sink and `findings`,
`clean`, `preflight`, `no-data` or `failed` as the state. The action uses the same names.
The successful report and its platform evidence select findings, clean or no-data; callers
cannot use a command name to bypass the corresponding evidence requirement.

Findings and coverage answer different questions: whether the tool found a notable change,
and how much of the intended benchmark scope it could judge. Findings take precedence.

| Successful evidence | Publication state |
| --- | --- |
| Any notable findings, including improvements in branch mode | `findings`, retaining any missing-platform or unjudged-series qualification |
| No findings, a fully judged nonempty analysis, and every intended platform completed | `clean` |
| No findings, but insufficient baseline, empty analysis scope, unjudged series or missing platforms | `no-data`, retaining the useful limited result and its explanation |
| Scope preflight explicitly selected no benchmarkable packages, so analysis did not run | `no-data`, explaining the empty scope |

For example, findings from a successful Linux collection still use `findings` when the Windows
collection failed. If the Linux analysis instead reports no findings, that missing Windows
coverage prevents a complete clean verdict and selects `no-data`.
Thus the partial result in `no-data` is not a set of hidden findings: it describes the judged
portion and why a complete verdict is unavailable. An omitted report is never an empty-scope
signal. `failed` records execution failure or cancellation, not a successful analysis outcome.

Comment findings, clean and no-data publication create or update the rolling comment.
Comment preflight seeds an owned placeholder or marks existing results stale; failed
publication changes only its own unfinished placeholder and never creates a comment.

Issue commands have distinct creation and update roles:

| Command | Creates an issue | Changes an existing open issue | Closes an issue | Reopens a closed issue |
| --- | --- | --- | --- | --- |
| `publish-issue-findings` | When no matching open rolling issue exists | Publishes eligible findings | Never | Never |
| `publish-issue-clean` | No | Publishes eligible all-clear, leaving the investigation open | Never | Never |
| `publish-issue-preflight` | No | Marks retained results stale and records pending work | Never | Never |
| `publish-issue-no-data` | No | Explains why recovery is unproven while retaining the previous report | Never | Never |
| `publish-issue-failed` | No | Retires only its own pending annotation, retaining the previous report | Never | Never |
| `alert` | When that project's run has no existing alert, open or closed | No | Never | Never |

Rolling-issue lookup considers open issues. A human-closed rolling issue stays closed; a later
findings publication can create a new rolling issue rather than reopening it. Alert lookup also
considers closed issues, so retrying an alert preserves a human-closed alert instead of
recreating it.
Non-creating rolling commands are logged no-ops when no open issue exists, but still validate
their inputs. Freshness and ownership guards can also preserve an existing issue unchanged.
No-data and failed annotations retain the report's measured commit and staleness.

## Publication evidence

Callers supply the JSON report and nonblank Markdown summary from the same successful
analysis pass and are responsible for keeping those outputs paired. The companion embeds
Markdown verbatim; the JSON supplies the named outcome, analysis mode, commit and coverage census.
The requested commit must match the report, which must describe an unmodified working tree;
unknown or inconsistent verdict/coverage metadata is an error rather than a default clean state.

The caller also supplies comma-separated identifiers for the expected collection platforms and
the platforms that completed successfully. Both lists must be nonempty. Completed platforms
must be a subset of expected platforms; duplicates and surrounding whitespace are normalized.
These are stable matrix identifiers, not arrays of runner-selection labels.

Findings and incomplete coverage are independent. A report with findings still publishes its
findings when a platform is missing, with a prominent warning naming completed and missing
platforms. Partial series coverage also remains visible. An issue may become all-clear only
when its history report is `clean`, every in-scope series was judged, and every expected
platform completed. Unjudged, empty, failed and partial runs cannot clear a regression
issue.

Delayed history reports do not overwrite or clear an issue describing a newer commit. When
the issue describes a different commit, replacement requires a verified forward comparison;
unrelated history or an unavailable comparison preserves the existing issue and reports why.
Delayed preflight also leaves a provably newer report unchanged rather than marking it stale
against an older frozen head. Unorderable commits retain the unknown-distance warning.

Report-bearing publication uses `--report-file`, the rendered `--body-file`, `--analyzed-sha`,
and `--expected-platforms` / `--completed-platforms`, identically for findings, clean and
no-data. The no-report no-data form requires an explicit empty-scope result and rejects
report-bearing inputs.
Report interpretation uses the existing JSON metadata rather than depending on the tool's
report types or adding a versioned report schema. Namespace resolution separately reuses
the core configuration and storage identity helpers.

## Run ownership

Run IDs identify workflow runs but do not order them. Attempt numbers establish precedence only
within the same run: a later attempt supersedes an earlier one.

Across distinct runs, the existing commit and live-head checks govern freshness. Publications
for the same commit follow serialized publication order, so an earlier-started run that finishes
later may replace another run's report for that commit. Starting a run does not reserve priority
over other runs.

Failed-state publication still requires the exact run ID, attempt and frozen head of the
unfinished placeholder or pending annotation. It cannot retire another run's work even when
both runs analyze the same commit.

## Workflow evidence

Workflow setup passes the action instance's project namespace to collection and analysis helpers.

`workflow-matrix` validates the requested platform CSV and emits the collection strategy
matrix, normalized expected-platform CSV, instance, and instance-qualified collection-job prefix.
The workflow derives its matrix jobs and later evidence inputs from those outputs rather than
maintaining separate platform lists or job-name conventions. Setup requires no repository or
GitHub credential.

Collection persists measurements to the configured store and emits an internal, versioned
receipt only after both collection and real machine-key capture succeed. Collection artifacts
contain only `receipt.json`; analysis reads measurements from the configured store for both
history and pull-request runs. A receipt binds repository, action instance, workflow run and
attempt, frozen analysis head, platform identifier and machine key. It is not an analysis report
or checksum manifest. Writing receipts and inspecting reports require neither a GitHub
credential nor an HTTP client.

Analysis preparation lists every job attempt for the run with Actions-read permission. The
collection job name is `cbh-collect:<instance>:<platform>`, either the entire name or its final
` / `-separated component in a reusable workflow. Other jobs and instances do not contribute.
Each expected platform must have an identifiable, terminal collection job. Only its latest
attempt can establish success, and success requires a matching receipt. A failed retry excludes
older successful collection from that platform; a platform not rerun retains its earlier
successful receipt. Unknown states, missing or ambiguous jobs and receipts, mismatched identities
and incomplete API discovery are errors.

At least one platform must succeed. Total failure does not create a report or authorize an
empty analysis. Preparation supplies the actual selected machine keys to the existing analyzer
recipe and reports platform coverage separately from the analyzer's series census.

Preparation writes machine-key files only for selected successful platforms. Collection receipts
establish job coverage, not measurement availability; the real analyzer determines whether the
configured store contains enough applicable data for a verdict. Receipt inputs accept only
ordinary artifact directories and regular receipt files. The machine-key destination must be
absent or empty and separate from receipt inputs and the workflow-output file. Existing unrelated
data is never deleted.

Report inspection projects the same validated evidence used by publication into workflow outputs,
including the publication state. Only findings are notable. Only a clean report with a full
series census and complete expected-platform coverage can authorize clean publication.
Issue all-clear additionally requires history mode. The workflow must not infer these decisions
from Markdown.

## Pull-request lifecycle

Preflight carries the frozen `--head`, `--run-id` and `--run-attempt`. A placeholder records
that ownership so failed publication from an older workflow or attempt cannot retire the new
placeholder. Terminal failure and empty-scope notes become new placeholders when benchmarking
is requested again.

Empty-scope no-data publication writes the explanatory note even when no comment exists.
Preflight and empty-scope publication require their frozen head to match the live head before
modifying the comment.

Publication checks the live head immediately before writing, after finding the existing
comment. If the PR has advanced, results receive a staleness warning. An existing comment
owned by the live head is preserved, whether it contains results, pending work or a terminal
note. When both the incoming report and the existing owned head are stale, a different incoming
head must be verified as a forward commit advance before replacing that state. Newer or
unorderable existing state is preserved. Distinct-run reports for the same stale commit still
follow serialized publication order. A failed live-head query produces a visible warning rather
than unqualified fresh-looking results.

## Identity

Workflow setup resolves the project namespace from the configured project ID. Workflows pass that
resolved namespace as internal instance data, including in receipts and collection-job identities;
it is not a consumer override.

Rolling issues use server-side `in:title` phrase search restricted to open issues in the
repository, excluding the date suffix from the query. Candidates must match the exact
project-qualified title form, not merely contain similar words. The companion reads the
selected issue by number before acting, rather than trusting indexed body/state data.
Its title prefix is reserved for this purpose and must be retained for discovery.
No issue labels or whole-repository body scans are needed.

PR comments still use project/kind markers within the one PR. Issue bodies retain run ownership,
report commit and status markers, but these are not the repository-wide search key. No path
adopts older output formats. A matching issue whose body cannot be interpreted is an explicit
error, not permission to overwrite it or create another.

Matching comments also require a recognized lifecycle state and coherent ownership/report
metadata. Missing, duplicated or contradictory state markers are errors that preserve the
existing comment, not permission to seed a new placeholder over it.

Multiple exact matches, incomplete searches and search failures are errors. Search indexing can
lag writes: issue creation is not an atomic upsert or an exactly-once guarantee. Ambiguous creates
receive bounded reconciliation reads; inability to establish success remains a failure without
another create. Known issue numbers use direct reads, avoiding unnecessary index dependence.

## One-off failure alerts

`alert` is separate from rolling-issue publication. Its title identifies the repository-scoped
project and workflow run ID. The run URL must identify the same
repository and run. Attempts share the run's alert; distinct failed runs receive distinct
issues. Existing alerts are left unchanged, including a human-closed alert found by title search over
closed as well as open issues. A retry neither recreates nor reopens it.

A successful run does not update or resolve earlier alerts. Human investigation owns their
disposition. Qualified findings and an alert can coexist when some collection platforms failed;
failed-state publication must not overwrite the successful qualified report.

## Authentication

The real adapter uses the GitHub REST API and accepts either of the environment names already
used by its callers. `GITHUB_TOKEN` supports workflow environments that export GitHub Actions'
job token. `GH_TOKEN` supports environments prepared for GitHub CLI use, including local
OAuth-authenticated tooling. The companion does not run `gh`, read its credential store or
mint a token; the caller supplies the credential in the environment.

The companion selects a nonblank `GITHUB_TOKEN` first and uses a nonblank `GH_TOKEN` only when
the former is absent or blank. If both are supplied, `GITHUB_TOKEN` wins. It does not switch to
the other token after an authentication failure or combine their permissions. This precedence
is the companion's own rule, not GitHub CLI's environment-variable ordering.
No personal access token or other long-lived credential fallback is introduced.
Analysis preparation requires Actions-read access; offline matrix setup, receipt creation and
report inspection do not read either credential variable.
The companion can run in the same job as Azure-backed analysis. The binary boundary separates
reporting responsibilities, not credentials; GitHub's token and the shared Azure identity
authenticate to their respective services.
