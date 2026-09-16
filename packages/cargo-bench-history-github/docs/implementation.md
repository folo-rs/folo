# Implementation

Pure marker and message composition is synchronous. Lifecycle orchestration is generic over the
`GitHub` port; unit tests use an in-memory fake and `futures::executor::block_on`, with no runtime,
network or real-time delay. The production `RestGitHub` adapter is the only HTTP boundary.

The port exposes semantic GitHub operations—list, create, update, close, delete, compare and read
the pull-request head—rather than a raw HTTP passthrough. Idempotent operations retry transient
failures in the adapter. Creates never retry blindly: orchestration reads by marker after an
error and treats a matching artifact as the successful result of the ambiguous request.

The binary is a thin Clap and Tokio entry point. `lib.rs` and `main.rs` contain only crate-level
documentation, attributes, re-exports and entry-point wiring.

## Evidence and state transitions

JSON decoding produces a validated analysis report before any publication operation. The
publication boundary validates the report's mode, frozen commit, outcome and census consistency
and separately carries collection-platform coverage. Message selection uses that typed evidence;
it does not scrape Markdown or map the tool's individual unjudged-reason vocabulary.

All-clear validation happens before issue lookup or mutation. This makes a contradictory report
or a missing platform an error even when the rolling issue happens not to exist. The CLI
supplies the same evidence to publish and cleanup, avoiding independent definitions of clean.

PR placeholder ownership combines workflow run ID and frozen head. Finalization acts only on its
own in-progress marker. Freshness and distance queries are distinct: inability to compute a
distance produces an explicit qualification, while a known newer result is preserved.
History issue replacement also checks commit ordering: the same commit or a verified forward
comparison may replace existing findings, while an absent, backward or unknown ordering leaves
them intact. Both publication and all-clear share this guard.
Serialized writers can still arrive with out-of-order frozen heads. Preflight preserves reports
proven newer by a reverse commit comparison; without a proved relationship it retains the
unknown-distance warning.

An ambiguous create is reconciled against both the artifact identity and the desired body.
Finding the same marker with different content is not proof that this publication committed;
the other content is preserved and the original failure remains visible.

## Explicit legacy adoption

Issue discovery separates marker-owned targets from explicitly eligible legacy targets. Both use
the complete open-issue list. Marker selection precedes title fallback; fallback requires an
exact title, the API author classification `user.type == "Bot"`, a unique candidate, and no
existing companion issue identity. Login spelling or body prose is not author evidence.

Only the explicitly selected legacy target bypasses the absent-SHA replacement guard. The
publication/all-clear boundary still validates report evidence first and writes the ordinary
identity/SHA body. Preflight preserves legacy content without installing the primary issue
identity, so a later validated adoption remains possible. Failure resolution adds its identity
before closing a directly adopted legacy issue. Create-error reconciliation remains marker-only
and still requires the desired body; migration does not authorize blind create retries.

`--legacy-issue-title TITLE` is a common option before `issue-preflight`, `publish-issue`,
`issue-cleanup`, `alert` or `resolve-alert`. It is rejected for other commands before credential
construction or output writing. `pr-comment-preflight` instead accepts the subcommand option
`--legacy-in-progress-marker '<!-- ... -->'`, validated with the same single-line HTML-comment
parser as `--comment-marker`. Migration configuration is passed through the existing lifecycle
context, without changing lifecycle function signatures.

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
Filesystem operations do not retry writes or clean existing destinations.

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
Inspection appends `outcome=<wire value>`, `notable=<bool>` and `can-clear=<bool>` in that order,
using lowercase booleans and the tool's existing outcome spelling.
