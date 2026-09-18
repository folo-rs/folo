# GitHub automation

The [benchmark-history action](https://github.com/folo-rs/cargo-bench-history-action)
exposes pipeline stages through one root composite action. It installs a tested tool
combination and delegates measurement, evidence validation and publication to the Rust
tools. Your workflow owns checkout, benchmark prerequisites, job dependencies,
concurrency and artifact upload/download.

## Prepare the repository

Commit `.cargo/bench_history.toml` with an explicit project ID and the storage configuration
you use for shared history. [Storage backends](storage.md) explains local and Azure storage;
[`setup-azure`](commands/setup-azure.md) provisions or exports the standard Azure resources.

Check out the measured commit with full history. PR comparisons use the real PR head and its
base, not the synthetic merge commit. Fork-origin PR execution is unsupported and is skipped
before credentialed operations. A source-built tool checkout is separate from the repository
being measured.

The action uses the job's ambient GitHub token for publication. Collection needs the normal
storage access; issue publication needs `issues: write`, and comment publication needs
`pull-requests: write`. Azure federation uses `id-token: write` and the non-secret client and
tenant IDs in the job environment. Do not add stored tokens or credentials to action inputs.

## Install the tested tools

Pin an action release or commit; `@v1` follows the tested releases of the first major version.
The action's release manifest selects its exact tool versions, without a caller version
override.

| Input | Purpose |
| --- | --- |
| `install-method` | `binstall` downloads published binaries with source fallback; `install` builds published packages; `path` builds a Folo source checkout. Default: `binstall`. |
| `source-path` | Folo checkout used by `path`. It supplies the complete required tool set, not independently selected binaries. |
| `working-directory` | The measured/configuration directory, separate from the installation source. |
| `config` | Optional configuration-file override; otherwise the tool uses `.cargo/bench_history.toml`. |

Each command uses the companion's action execution boundary. Collection, analysis and backfill
also use the main tool. Test-only binaries are not installed for consumer invocations.
Released installations may use the installed-binary cache; `path` always builds the selected
source instead of restoring a released executable.

Path mode is for a source/action combination tested together. A shared source checkout alone
does not establish compatibility with an older action interface.

## Compose the stages

`command` selects one operation. Inputs that do not apply to that operation are errors.
The action always emits explanatory diagnostics.

| Command | Responsibility |
| --- | --- |
| `collect` | Measure the selected packages and store their results; return the actual `machine-key`. |
| `backfill` | Collect the inclusive `from`/`to` history range without analyzing or publishing. |
| `analyze-history` | Analyze the selected history commit, using it as both context and base. |
| `analyze-pr` | Compare the measured PR context with its base. |
| `publish-issue-*` | Maintain the rolling history issue with validated findings, clean, preflight, no-data or failed state. |
| `publish-comment-*` | Maintain the PR's rolling comment with the corresponding state. |
| `alert` | Create a one-off workflow-failure issue for the run; successful later runs do not close it. |

Collection and backfill accept comma-separated `packages`, `exclude`, `bench` and `features`,
plus Cargo feature flags and `best-of`. Empty package scope means the whole workspace at this
lower-level action boundary; a workflow that selected no benchmarkable packages must route to
explicit empty-scope publication instead of invoking collection.

`on-existing` selects the immutable-storage collision policy. Collection defaults to `error`;
`skip` preserves stored points, and `overwrite` deliberately replaces them. Backfill defaults
to `skip` and also accepts `overwrite`. [Measurement stability](concepts/stability.md) explains
why comparable runs must use a consistent protocol.

Analysis receives the collected `machine-keys` directory and explicit `expected-platforms`
and `completed-platforms` lists. In a matrix, only validated successful collection legs supply
those keys and completed platforms. A hardware key is not proof of which workflow produced
every stored observation.

Analysis returns the outcome, notable flag, regression count, platform qualification and paths
to the full Markdown, JSON and condensed summary. It does not post them. Upload the reports,
then pass the summary, JSON, analyzed SHA, artifact URL and platform evidence to the selected
publication command. Paths are local to the analysis job, not cross-job artifact identifiers.

## Keep conclusions qualified

Use the [analysis outcome](appendix/reporting.md#analysis-outcomes), not only `notable`, to
select a successful publication state. Findings remain findings under incomplete coverage.
A complete all-clear requires both a fully judged analysis and complete collection-platform
evidence. An inconclusive analysis or empty scope is `no-data`; execution failure is not an
analysis outcome.

Preflight and terminal commands carry the frozen head, run ID and attempt. A failed command
retires only its own unfinished state. Attempts are ordered only within one run; distinct runs
use commit/live-head freshness, with same-commit publication following serialized arrival.
Reports for an older head preserve comments owned by the verified current head, including
pending and terminal notes.

Publication requires the rendered summary and JSON from the same analysis pass. A missing or
whitespace-only summary is an error. Titles, markers, wording and lifecycle behavior are fixed;
the internal namespace comes from the core's canonical project identity.

## Failures and release availability

An unavailable companion or required context can prevent an alert. The workflow remains failed;
there is no independent fallback publisher. Workflows must not turn failed collection into a
clean analysis or treat missing evidence as an empty scope.

The action repository's required installation check proves exact registry and prebuilt
availability before an action release. It uses fresh installation roots and does not accept
cache hits or source fallback as proof of a promised archive. After a monorepo tool release,
the paired action PR's author reruns that check; publication does not rerun it automatically.
