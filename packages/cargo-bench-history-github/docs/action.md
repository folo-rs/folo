# Action execution

This is the unsupported interface between the separately versioned root action and its
installed companion. An action release pins the tested binary combination.

```text
cargo-bench-history-github action --inputs-file PATH --github-output PATH --temp-dir PATH [--tool PATH]
```

Every root command uses this companion. `collect`, `backfill`, `analyze-history` and
`analyze-pr` additionally use the main executable selected by `--tool`, or
`cargo-bench-history` on `PATH` when omitted. Publication does not launch the main executable.

## Input file and paths

The input file is a JSON object with unique keys and string values. Empty strings mean
unspecified. Unknown keys, non-string values, nonblank inapplicable inputs and invalid
combinations are errors. Inputs are single-line; list inputs are comma-separated, with
surrounding item whitespace accepted. Boolean values are exactly `true` or `false`.

`--inputs-file` resolves relative to the invocation directory. The optional JSON
`working-directory` also resolves from that directory and defaults to it. All other relative
paths, including `config`, storage, cache, machine keys, publication reports and the remaining
CLI paths, resolve against the measured working directory. The bootstrap's `source-path`
has no role in selecting the measured checkout.

The common JSON inputs are `command`, `working-directory` and `config`. Omitted `config`
uses the core `.cargo/bench_history.toml` discovery and project-directory fallback.
Neither `install-method` nor `source-path` is accepted; the bootstrap owns both.
There is no consumer namespace override.

## Command-specific inputs

**Report inputs** are required `body-file`, `report-file`, `analyzed-sha`,
`expected-platforms`, `completed-platforms`, and optional `artifact-url`.
`body-file` is the tool-rendered condensed summary; `report-file` is the JSON from the
same successful analysis. Existing evidence validation determines which named publication
state is legal. Empty-scope publication rejects report inputs and packages.

**Run ownership** is `run-id` and `run-attempt`, both positive integers. `pr-number`
is also positive. Failed publication requires `conclusion=failure` or `cancelled`;
there is no success default.

| Command | Inputs |
| --- | --- |
| `collect` | `local-path`, `packages`, `exclude`, `bench`, `best-of`, `on-existing`, `all-features`, `no-default-features`, `features` |
| `backfill` | Collection inputs plus required `from`, `to`, and optional `ignore-errors` |
| `analyze-history` | `local-path`, `cache`, required `machine-keys`, `context`, `since`, required `expected-platforms`, `completed-platforms` |
| `analyze-pr` | The history inputs except `since`, plus optional `base` |
| `publish-comment-findings`, `publish-comment-clean` | Report inputs, run ownership, `pr-number`, required `packages` |
| `publish-issue-findings`, `publish-issue-clean` | Report inputs and run ownership |
| `publish-comment-preflight` | Run ownership, `head`, `pr-number`, required `packages` |
| `publish-issue-preflight` | Run ownership and `head` |
| `publish-comment-no-data` | Run ownership and `pr-number`; either report inputs and required `packages`, or `empty-scope=true` and `head` |
| `publish-issue-no-data` | Run ownership; either report inputs, or `empty-scope=true` and `head` |
| `publish-comment-failed` | Run ownership, `pr-number`, `head`, `run-url`, required `conclusion` |
| `publish-issue-failed` | Run ownership, `head`, `run-url`, required `conclusion` |
| `alert` | `run-id`, `run-url` |

Collection defaults to workspace scope, including any exclusions. Explicit `packages`
conflicts with `exclude`. `bench`, packages, exclusions and features become repeated core
flags. `all-features` defaults to `true` for collection and backfill;
`no-default-features` and `ignore-errors` default to `false`; `best-of` defaults to `1`.
`on-existing` defaults to `error` for collection and `skip` for backfill.
Collection accepts `error`, `skip` and `overwrite`. Backfill accepts only `skip` and
`overwrite`; skip passes no core write-mode flag.

Analysis defaults `context` to `HEAD` and resolves it to a full commit SHA.
History passes that SHA as both context and base. PR analysis passes an explicit `base`
when supplied, otherwise leaves base selection to the core. Its report must be branch mode.
Analysis always passes `--engine all --target-triple all --no-dirty --no-text --verbose`.
`local-path` conflicts with `cache`. Other main work also enables verbose diagnostics.

`machine-keys` is a directory containing ordinary `machine-key.txt` files, directly or in
ordinary subdirectories. Each file contains one actual 16-hex-digit fingerprint;
surrounding whitespace is accepted, case normalized and duplicate keys deduplicated.
At least one key is required. Other files and filesystem links within this tree are errors.
Expected and completed platform CSVs are required independently of these keys and use the
existing platform-coverage validation.

## Execution context and skips

Publication obtains its repository from `GITHUB_REPOSITORY`, or the event's repository.
Explicit execution inputs take precedence. Missing `run-id` and `run-attempt` may use
`GITHUB_RUN_ID` and `GITHUB_RUN_ATTEMPT`. `pr-number` may use the pull-request event.
`head` may use the event's real PR head; outside PR events it may use `GITHUB_SHA`.
It never substitutes a synthetic merge SHA for a PR head.

A missing `run-url` may be formed from `GITHUB_SERVER_URL`, the known repository and run ID.
The existing lifecycle URL validation still applies. No run IDs, attempts, conclusions or
repository names are fabricated.

The event named by `GITHUB_EVENT_PATH` establishes the same-repository gate.
`pull_request` and `pull_request_target` require PR event identity. Fork-origin PRs,
including unavailable source repositories, emit an explanatory log and
`skipped=true`, `skip-reason=fork-pull-request` without benchmark, storage or publication
work. Shape validation and local namespace resolution still apply.

For offline commands, the companion does not read `GITHUB_TOKEN` or `GH_TOKEN` or
initialize GitHub authentication. Core and Git child processes inherit the caller's environment
unchanged, including variables needed by build helpers or benchmarks. The credentialed adapter
is constructed only for publication, after input and fork checks.

## Artifacts and outputs

`--temp-dir` is an existing temporary root. Analysis validates full Git history without
fetching; shallow checkouts must use `fetch-depth: 0`. It creates a unique owned report
directory beneath the temporary root, outside the canonical checkout. A supplied cache
must also be outside the checkout. These locations avoid dirtying measured source.

Reports persist after process exit. The owned directory contains `report.md`, `report.json`,
`summary.md` and `outcome.txt`, ready for a later job-local artifact upload. The action does
not clean them or upload them itself.

Successful invocations append `instance=<canonical project namespace>` to `--github-output`.
Collection additionally appends `machine-key=<fingerprint>`, only after both collection and
the dedicated machine-key command succeed.

Analysis appends the shared evidence projection `outcome`, `notable`, `can-clear` and
`publication-state`, plus `partial-platform-coverage`, `regressions`, `report-markdown`,
`report-json` and `report-summary`. Booleans are lowercase; report paths are absolute
and local to this job. The outcome file must agree with the validated JSON, and rendered
reports must be nonblank. No success outputs are appended on work or evidence failure.

Windows report outputs use ordinary paths for artifact-upload compatibility. Temporary locations
that cannot be represented in this supported form are rejected.

`--github-output` preserves earlier records, including a preceding record without its
final newline. The caller supplies a regular output file, or an absent file with an
existing parent directory.
