# Implementation

Pure marker and message composition is synchronous. Lifecycle orchestration is generic over the
`GitHub` port; unit tests use an in-memory fake and `futures::executor::block_on`, with no runtime,
network or real-time delay. The production `RestGitHub` adapter is the only HTTP boundary.

The port exposes semantic GitHub operations—title search, issue reads, comment listing, create,
update, compare and read the pull-request head—rather than a raw HTTP passthrough. Idempotent
operations retry transient failures in the adapter. Creates never retry blindly: orchestration
reconciles the intended identity and content after an error.

The binary is a thin Clap and Tokio entry point. `lib.rs` and `main.rs` contain only crate-level
documentation, attributes, re-exports and entry-point wiring.

The file-backed online command dispatcher accepts the GitHub port and clock independently of
process setup. Native integration scenarios use the same dispatcher with ordinary report files,
an in-memory GitHub port and a frozen clock. They observe publication snapshots after each command,
so argument wiring and terminal transitions are covered without credentials or network writes.

## Evidence and state transitions

JSON decoding produces a validated analysis report before any publication operation. The
publication boundary validates the report's mode, frozen commit, outcome and census consistency
and separately carries collection-platform coverage. Message selection uses that typed evidence;
it does not scrape Markdown or map the tool's individual unjudged-reason vocabulary.

All-clear validation happens before issue lookup or mutation. This makes a contradictory report
or a missing platform an error even when the rolling issue happens not to exist. The CLI
supplies the same evidence to findings, clean and no-data publication, avoiding independent
definitions of clean. Shared validation projects a publication state for workflow dispatch and
rechecks it at the named command boundary. An explicit empty-scope variant carries no report;
it cannot be confused with a missing or malformed artifact.

Pending ownership combines workflow run ID, attempt and frozen head. Failed-state publication
acts only on its own in-progress placeholder or issue annotation. Freshness and distance queries
are distinct: inability to compute a distance produces an explicit qualification, while a known
newer result is preserved.
History issue replacement also checks commit ordering: the same commit or a verified forward
comparison may replace existing findings, while an absent, backward or unknown ordering leaves
them intact. Both publication and all-clear share this guard.
Serialized writers can still arrive with out-of-order frozen heads. Preflight preserves reports
proven newer by a reverse commit comparison; without a proved relationship it retains the
unknown-distance warning.

Issue no-data and failed states share the bounded annotation mechanism rather than replacing
the previous report. They retain its analyzed-commit identity and freshness qualification.
Preflight records ownership so delayed terminal work cannot retire a newer pending annotation.
No-data at the pending head can retire that annotation despite an unknown distance to the
retained report, including when a successful preflight is reused by a later run attempt.
Absence of an issue is diagnosed after input validation; only findings can create one.

An ambiguous create is reconciled against both the artifact identity and the desired body.
Finding the same identity with different content is not proof that this publication committed;
the other content is preserved and the original failure remains visible.

## Report identity and composition

Issue discovery sends a repository-scoped, quoted `in:title` query to GitHub's search endpoint.
It filters candidate metadata by the exact project-qualified title form, without using the
rolling date suffix as identity. The selected number is read through the ordinary issue
endpoint so stale indexed content does not drive a mutation. Comment discovery matches
instance/kind markers only within the target PR. No operation scans all repository issue bodies
or relies on author identity, and there is no title-renaming fallback or migration path.
Comment identity alone does not permit a lifecycle transition without interpretable
run-attempt ownership.

The search adapter validates `total_count`, `incomplete_results` and pagination before treating
the result set as complete. It URL-encodes the query and treats project text as a literal search
value, not additional qualifiers. Multiple exact matches, the search result cap and failed or
incomplete discovery surface explicitly. An empty index result does not prove the outcome of
an ambiguous create; bounded reconciliation may still end with the original error. Serializing
issue writers prevents ordinary concurrent creation, not search-index lag.
Search rate limits use the adapter's bounded retry policy; authorization and query-validation
errors are not treated as empty results or transient indexing delays.

The lifecycle context carries repository, internal instance namespace and verbosity only.
Message composition derives all markers from that namespace and owns the standard titles,
advisory wording and documentation link. An injected clock supplies the UTC date captured for
an issue body update, and retries reuse that date. Title and body are sent together in the
same issue update. No-op paths do not refresh titles; measured-commit freshness remains in
the body, independently of the title date. Publication passes the optional artifact URL directly
as report data alongside validated evidence and the tool-rendered summary.

The clock is `tick::Clock`; calendar conversion uses Jiff with the UTC time zone, never the
host's local zone. Tests inject frozen instants. Report metadata and the bounded annotation
are parsed separately, so a pending run's ownership never replaces the retained report's
ownership or analyzed commit. Malformed and duplicated identity metadata are errors.

Clean issue publication only updates the all-clear body. There is no issue-closing operation
in the companion. Empty-scope comment publication shares ordinary update/create reconciliation
and always writes the explanatory note.

One-off alert discovery searches its exact project/run-qualified title and includes closed issues.
Existing alerts are not updated or reopened. An ambiguous create still requires the exact
intended body as well as identity to establish success; a different body discovered after
the failed request is not evidence that the request committed. Distinct run IDs do not
aggregate into a shared issue, and run attempts do not create additional alert identities.

## HTTP adapter

The semantic port is implemented by a REST adapter over an injected request executor and
delay provider. Request construction, JSON decoding, pagination, status classification and
retry decisions run identically with the real executor and a scripted in-memory executor.
Tests record serialized requests and requested delays without using sockets or a clock.

Idempotent reads and updates retry only transient failures within a bounded attempt and delay
budget. An acceptable `Retry-After` delay is honored; unsupported or excessive delays are
reported rather than retried prematurely. Pagination must make progress, and a later page's
failure remains an error rather than a successful partial list. Comparison distances are
numeric only for a verified linear forward relationship or identical commits.

The reqwest boundary disables automatic redirects and retries so they cannot bypass the
adapter's operation-specific policy. Creates have no blind retry; lifecycle reconciliation
uses the same REST decoding as ordinary lookup. Credentials are redacted from diagnostic
representations. Only the actual network and timer primitives are outside in-process tests;
the request and response policy is not excluded with them.

## Workflow evidence adapters

Offline matrix setup and job reconciliation share platform validation and the instance-qualified
collection-job namespace. Setup emits both the strategy matrix and expected-platform CSV from
the same validated, sorted set, keeping workflow orchestration free of duplicate parsing rules.

Receipt decoding and job reconciliation operate on in-memory values. Filesystem adapters retain
artifact paths outside the receipt model, then materialize only the selected indices after all
identities and latest-attempt decisions have been validated. Local object merging compares bytes
before writing anything, preserving ordinary relative store paths without a second storage format.
Reserved LocalStorage atomic-write temporary files are not objects and are omitted from merging.
Filesystem operations do not retry writes or clean existing destinations.

One ordered platform/attempt index owns both receipt association and latest-job lookup.
Duplicate attempts are rejected before selection, so there is no equal-attempt tie-breaker.
Verbose diagnostics are projected in memory before the stderr adapter emits them, allowing
complete and partial collection qualifications to be exercised without capturing process output.

Job listing uses `GET /repos/{owner}/{repo}/actions/runs/{run_id}/jobs` with `filter=all`,
`per_page` and `page` on every request. Its `{total_count, jobs}` envelope differs from issue and
comment lists. Stable counts, unique job IDs and complete pagination are required. Job identity
includes positive `run_id` and `run_attempt`; API `head_sha` is deliberately not an analysis-head
check because GitHub can use a PR merge ref. Receipts bind the explicitly frozen analysis commit.
The scripted HTTP executor tests real request construction and response decoding without sockets,
credentials or real delay.

The `private-test-util` feature exposes a deliberately unsupported preparation entry point for
native integration tests. It injects already-discovered job records and bypasses only HTTP; real
artifact traversal, selection, fresh destinations, ordinary object copying and workflow outputs
execute unchanged. Offline commands also run through the binary without credential environment
variables. This keeps real filesystem and process coverage outside the unit/Miri harness.

### Command and artifact contract

Common options precede the subcommand: `--repository owner/name`, `--instance ID`, `--verbose`.
The internal instance input carries the configured project ID supplied by workflow setup, not a
consumer-selected action override. Omitting it retains the companion's `default` namespace.
Receipt creation and analysis preparation default the repository from `GITHUB_REPOSITORY`.
Matrix setup and report inspection need neither a repository nor a GitHub credential.

```text
cargo-bench-history-github --instance folo workflow-matrix
  --platforms CSV --github-output PATH

cargo-bench-history-github --repository owner/name --instance folo collection-receipt
  --run-id N --run-attempt N --head SHA --platform ID
  --machine-key-file PATH --file ARTIFACT_ROOT\receipt.json

cargo-bench-history-github --repository owner/name --instance folo prepare-analysis
  --run-id N --head SHA --expected-platforms CSV
  --receipts-dir DOWNLOAD_ROOT --machine-key-dir KEY_ROOT --github-output PATH
  [--local-results-dir RESULTS_ROOT]

cargo-bench-history-github inspect-report
  --report-file PATH --analyzed-sha SHA --expected-platforms CSV
  --completed-platforms CSV --github-output PATH
```

These examples wrap arguments for readability, not shell execution. SHA is a full, clean
hexadecimal commit ID. Run IDs and attempts are positive. Machine-key files contain the actual
16-hex-digit fingerprint, with surrounding command-output whitespace accepted and hexadecimal
letters normalized to lowercase. Matrix, receipt and preparation platform identifiers use ASCII
letters, digits, `.`, `_` and `-`, excluding `.` and `..` as entire identifiers.

Lifecycle commands use the same common options and repository fallback. Rolling commands carry
`--run-id N --run-attempt N`; report-bearing commands use this shared evidence group:

```text
--body-file PATH --analyzed-sha SHA
  --report-file PATH --expected-platforms CSV --completed-platforms CSV [--artifact-url URL]
```

| Command | Additional inputs |
| --- | --- |
| `publish-comment-findings` | Report evidence, `--pull-request N --packages CSV` |
| `publish-comment-clean` | Report evidence, `--pull-request N --packages CSV` |
| `publish-comment-preflight` | `--pull-request N --packages CSV --head SHA` |
| `publish-comment-no-data` | `--pull-request N`, plus either report evidence and `--packages CSV`, or `--empty-scope --head SHA` |
| `publish-comment-failed` | `--pull-request N --head SHA --run-url URL --conclusion failure\|cancelled` |
| `publish-issue-findings` | Report evidence |
| `publish-issue-clean` | Report evidence |
| `publish-issue-preflight` | `--head SHA` |
| `publish-issue-no-data` | Either report evidence, or `--empty-scope --head SHA` |
| `publish-issue-failed` | `--head SHA --run-url URL --conclusion failure\|cancelled` |

`alert --run-id N --run-url URL` uses no report or attempt ownership. Its validated run
identity determines the one-off title; the URL names that same repository and run.

Matrix setup appends these outputs, with one sorted platform set shared by JSON and CSV:

```text
matrix={"platform":["linux","windows"]}
expected-platforms=linux,windows
instance=folo
collection-job-prefix=cbh-collect:folo
```

The collection strategy consumes `matrix`; job names append `:<platform>` to
`collection-job-prefix`. Later evidence commands consume `expected-platforms` and `instance`
unchanged. The prefix has no trailing separator.

Download artifacts into separate immediate child directories of `DOWNLOAD_ROOT`, without merging
artifact contents:

```text
DOWNLOAD_ROOT\
  artifact-for-linux\
    receipt.json
    results\                 optional; ordinary local store object paths below here
  artifact-for-windows\
    receipt.json
    results\
```

Artifact roots contain only `receipt.json` and optional `results`. Distinct historical attempts
can be present; duplicate receipts for the same platform and attempt are rejected. History
artifacts need only the receipt. The receipt JSON has `version`, `repository`, `instance`,
`run_id`, `run_attempt`, `head`, `platform` and `machine_key` fields; unknown fields or versions
are errors.

Preparation writes `KEY_ROOT\<platform>\machine-key.txt`, compatible with the existing recursive
key-directory recipe. It appends single-line outputs in this order:

```text
completed-platforms=linux,windows
machine-keys=0123456789abcdef
complete=true
```

Platform and deduplicated key lists are sorted. `complete` measures platform coverage only.
The optional results destination exists even when selected collection produced no objects.
Both destination directories must be absent or empty. `GITHUB_OUTPUT` must be a separate regular
file with an existing parent directory; output appending preserves earlier workflow values.
Inspection appends `outcome=<wire value>`, `notable=<bool>`, `can-clear=<bool>` and
`publication-state=findings|clean|no-data`, using lowercase booleans and the tool's existing
outcome spelling. `can-clear` retains its history-only meaning; `publication-state` also
serves comment publication. The latter is a projection of existing report/platform evidence,
not another tool verdict or a caller override.
