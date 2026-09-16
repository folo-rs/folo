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
* retires pull-request placeholders after failure or when nothing benchmarkable changed; and
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

Publication uses `--report-file` plus `--expected-platforms` and `--completed-platforms`.
`publish-issue` and `publish-pr-comment` additionally take the rendered `--body-file` and
`--analyzed-sha`; `issue-cleanup` verifies the same JSON against `--clean-commit`.
The report's existing JSON metadata is the integration boundary, not a new versioned report
schema or a dependency on the tool's private implementation packages.

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

## Authentication

The real adapter reads `GITHUB_TOKEN`, falling back to `GH_TOKEN`, and uses the GitHub REST API.
No personal access token or other long-lived credential is introduced.
