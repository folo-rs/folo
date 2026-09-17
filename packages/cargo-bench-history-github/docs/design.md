# Design

`cargo-bench-history-github` is the unsupported GitHub automation companion for
`cargo-bench-history`. It owns the GitHub-specific envelope and lifecycle around reports while
the main tool remains independent of GitHub.

The complete action design lives in
[`../../cargo-bench-history/docs/reusable-action.md`](../../cargo-bench-history/docs/reusable-action.md).
This package implements its GitHub lifecycle and workflow-evidence responsibilities.
It deliberately has no stable API
or command-line contract; the separately versioned action pins a tested companion version.

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
the companion's last body update, not the last measurement. No-op operations leave it unchanged.
Workflow-failure issues use `Benchmark history workflow failed for <project> (run <run-id>)`.
The caller supplies report artifacts and evidence, not titles, introductions, documentation
links or comment identities.

Regression issues remain open after all-clear so their rolling history is retained. Failure
alerts describe individual workflow runs and stay unchanged when later runs succeed.
Empty pull-request scope always creates or updates an explanatory note rather than deleting
the rolling comment.

## Publication states

Commands use `publish-<sink>-<state>`, with `comment` or `issue` as the sink and `findings`,
`clean`, `preflight`, `no-data` or `failed` as the state. The action uses the same names.
The successful report and its platform evidence select findings, clean or no-data; callers
cannot use a command name to bypass the corresponding evidence requirement.

`findings` retains findings even when coverage is partial. `clean` requires a fully judged,
nonempty analysis and complete intended-platform coverage. `no-data` means no complete verdict
is available, not necessarily that no measurements exist: its message includes the actual
insufficient-baseline, unjudged-series or missing-platform explanation and any useful partial
result. An explicit empty-scope input covers preflight selecting no benchmarkable packages
without running analysis; omission of report evidence alone is never that signal. `failed`
records failure or cancellation rather than inventing an analysis outcome.

Comment findings, clean and no-data publication create or update the rolling comment.
Comment preflight seeds an owned placeholder or marks existing results stale; failed
publication changes only its own unfinished placeholder and never creates a comment.

Only issue findings publication creates a rolling regression issue. Issue clean publishes
all-clear; preflight marks a pending run; no-data explains why recovery could not be established;
failed retires only its own pending annotation. No-data and failed preserve the previous report,
its measured commit and staleness. These issue commands are a logged no-op when no issue is
open, but still reject invalid evidence. Their role is to keep an existing investigation
accurate, not create issues merely to announce workflow status.

## Publication evidence

Result publication consumes the JSON report and Markdown summary from the same successful
analysis pass. The JSON supplies the named outcome, analysis mode, commit and coverage census.
The requested commit must match a clean report; unknown or inconsistent verdict/coverage
metadata is an error rather than a default clean state.

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
The report's existing JSON metadata is the integration boundary, not a new versioned report
schema or a dependency on the tool's private implementation packages.

## Workflow evidence

`workflow-matrix` validates the requested platform CSV and emits the collection strategy
matrix, normalized expected-platform CSV, instance, and instance-qualified collection-job prefix.
The workflow derives its matrix jobs and later evidence inputs from those outputs rather than
maintaining separate platform lists or job-name conventions. Setup requires no repository or
GitHub credential.

Collection emits an internal, versioned receipt only after both collection and real machine-key
capture succeed. A receipt binds repository, action instance, workflow run and attempt, frozen
analysis head, platform identifier and machine key. It is not an analysis report or checksum
manifest. Writing receipts and inspecting reports require neither a GitHub credential nor an
HTTP client.

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

For PR analysis, only selected platforms' ordinary local result trees are composed into a fresh
run-local input directory. Identical copies of an object are accepted; differing bytes at the
same relative path are an error. Receipt files and failed-platform results never enter this
directory. A successful collection with no objects may omit its results tree or supply an empty
one; the real analyzer determines the resulting nothing-in-scope outcome. Input traversal accepts
only ordinary directories and regular files, and destinations must be absent or empty, separate
from inputs and from each other. Existing unrelated data is never deleted.

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
comment. If the PR has advanced, results receive a staleness warning; if fresh results for the
live head already exist, the older publication leaves them untouched. A failed freshness query
produces a visible warning rather than unqualified fresh-looking results.

## Identity

Identities derive from the configured project ID. Workflows pass that namespace as internal
instance data, including in receipts and collection-job identities; it is not a consumer override.

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

The real adapter reads `GITHUB_TOKEN`, falling back to `GH_TOKEN`, and uses the GitHub REST API.
No personal access token or other long-lived credential is introduced.
Analysis preparation requires Actions-read access; offline matrix setup, receipt creation and
report inspection do not read either credential variable.
The companion can run in the same job as Azure-backed analysis. The binary boundary separates
reporting responsibilities, not credentials; GitHub's token and the shared Azure identity
authenticate to their respective services.
