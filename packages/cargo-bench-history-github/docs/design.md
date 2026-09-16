# Design

`cargo-bench-history-github` is the unsupported GitHub automation companion for
`cargo-bench-history`. It owns the GitHub-specific envelope and lifecycle around reports while
the main tool remains independent of GitHub.

The complete action design lives in
[`../../cargo-bench-history/docs/reusable-action.md`](../../cargo-bench-history/docs/reusable-action.md).
This package implements that document's report-sink commands. It deliberately has no stable API
or command-line contract; the separately versioned action pins a tested companion version.

## Responsibilities

The companion:

* publishes and updates the rolling regression issue and pull-request comment;
* marks an existing report stale while a new benchmark run is in flight;
* replaces a recovered regression issue with an all-clear state and optionally closes it;
* reports and resolves workflow failures;
* retires pull-request placeholders after failure or when nothing benchmarkable changed;
* binds successful collection to actual workflow job attempts and measured machine keys; and
* reconciles an ambiguous create by looking up the hidden identity marker before retrying.

It embeds the Markdown summary rendered by `cargo-bench-history` verbatim. It never interprets
findings or re-derives analysis vocabulary.

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
platform completed. Unjudged, empty, failed and partial runs cannot clear or close a regression
issue.

Delayed history reports do not overwrite or clear an issue describing a newer commit. When
the issue describes a different commit, replacement requires a verified forward comparison;
unrelated history or an unavailable comparison preserves the existing issue and reports why.
Delayed preflight also leaves a provably newer report unchanged rather than marking it stale
against an older frozen head. Unorderable commits retain the unknown-distance warning.

Publication uses `--report-file` plus `--expected-platforms` and `--completed-platforms`.
`publish-issue` and `publish-pr-comment` additionally take the rendered `--body-file` and
`--analyzed-sha`; `issue-cleanup` verifies the same JSON against `--clean-commit`.
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

Report inspection projects the same validated evidence used by publication into workflow outputs.
Only findings are notable. Only a clean history report with a full series census and complete
expected-platform coverage can authorize all-clear. The workflow must not infer these decisions
from Markdown.

## Pull-request lifecycle

Preflight carries the frozen `--head` and `--run-id`. A placeholder records that ownership
so a finalizer from an older workflow cannot retire the new run's placeholder. Terminal failure
and empty-scope notes become new placeholders when benchmarking is requested again.

Empty-scope cleanup writes the explanatory note even when no comment exists. The explicit
delete option remains a no-op when nothing exists. Preflight and cleanup require their frozen
head to match the live head before modifying the comment.

Publication checks the live head immediately before writing, after finding the existing
comment. If the PR has advanced, results receive a staleness warning; if fresh results for the
live head already exist, the older publication leaves them untouched. A failed freshness query
produces a visible warning rather than unqualified fresh-looking results.

## Identity

Every rolling artifact carries a hidden marker derived from the action instance and artifact
kind. Issues distinguish `regression` from `failure-alert`; pull requests carry one
`pr-comment` artifact per instance. Displayed titles are not identities and may be edited.

No issue labels are applied. Rolling issues are found by enumerating open issues and matching
the hidden marker in the body.

`--comment-marker` may select an existing PR comment's exact, single-line HTML marker.
Run ownership, status and staleness markers remain scoped to `instance`. Issue commands reject
the PR-only marker override.

## Explicit legacy adoption

Issue commands may opt into `--legacy-issue-title TITLE`. Current instance/kind markers always
take precedence. Only when no current marker matches may the companion select an open,
bot-authored issue with that exact title. Multiple eligible issues are an error, as is a
title-selected issue carrying another companion issue identity. Human-authored issues are not
eligible. There is no title fallback without this option, and PR/evidence commands reject it.

Preflight may add a staleness warning while preserving the legacy report and its title. It does
not claim the issue's primary identity before a validated report is available. Publication or
all-clear then installs the current identity and the validated analysis SHA on that same issue;
initial adoption does not require interpreting a legacy commit summary. Once adopted, ordinary
marker lookup and commit-order safeguards govern subsequent writes, including when the option
remains configured. All-clear still requires clean, complete history evidence.

The failure-alert commands use the same opt-in selection. Alert publication adopts the selected
issue with its standard failure body. Resolution can directly adopt and close a legacy failure
issue while retaining its original failure details. The caller supplies the appropriate legacy
title for each issue kind and serializes issue writers.

PR preflight may take `--legacy-in-progress-marker '<!-- ... -->'`. When the selected rolling
comment contains this exact HTML comment line, preflight replaces it with the current run's
owned placeholder instead of treating it as results. The normal live-head check still applies,
the selected rolling-comment identity is retained, and finalization requires the new run/head
ownership marker. No human prose or legacy commit-summary formatting is parsed.

## Authentication

The real adapter reads `GITHUB_TOKEN`, falling back to `GH_TOKEN`, and uses the GitHub REST API.
No personal access token or other long-lived credential is introduced.
Analysis preparation requires Actions-read access; offline matrix setup, receipt creation and
report inspection do not read either credential variable.
