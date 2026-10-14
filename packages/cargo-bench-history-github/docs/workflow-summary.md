# Workflow results cards

The companion renders a compact workflow result independently of GitHub publication:

```text
cargo-bench-history-github workflow-summary --inputs-file INPUT.json --output-file CARD.md
```

This offline command needs no GitHub credentials or repository checkout. It creates a new
Markdown file and refuses to overwrite an existing file. Input and output arguments resolve
from the invocation directory; evidence paths inside the JSON resolve from the input file's
parent directory. Absolute evidence paths are also accepted. The command does not write to
`GITHUB_STEP_SUMMARY`, publish a post, run analysis or alter storage.

The separately versioned action owns orchestration: select one final card per invocation,
append the complete generated Markdown once to `GITHUB_STEP_SUMMARY`, and retain diagnostic
logs and the original job result. Summary generation succeeding does not turn failed,
cancelled or timed-out work into success. A renderer failure remains visible and must not
replace a benchmark error with a successful fallback.

Preparation and installation can fail before the companion is available. The action's minimal
fallback must run without that binary: show execution failure/cancellation/skip, the fact that
no validated analysis is available, and the run/log link. Do not duplicate verdict or coverage
rules in YAML or PowerShell. An `always()` finalization path can only run while GitHub still
provides a runner; hard cancellation may prevent any final write.

## Input contract

The strict JSON object contains `execution`, `publication`, `run_url`, optional `artifact_url`,
and `result`. Unknown fields and enum values are errors. Links must be HTTP(S) URLs without
embedded credentials. Artifact links identify uploaded report or completion bundles, not local
paths. No artifact link is inferred from the existence of a file.

`execution` is `success`, `failure`, `cancelled` or `skipped`. It describes the workflow work
being summarized, not the analyzer's verdict. A failed matrix can still have successful analysis
of completed platforms. The action must preserve that distinction.

`publication` is independent:

| Value | Meaning |
| --- | --- |
| `disabled` | Issue/comment publication was disabled; collection and storage remain independent |
| `completed` | The publication request completed; lifecycle guards or an absent rolling issue may legitimately leave posts unchanged |
| `failed` | Publication failed |
| `not_attempted` | No publication request ran |
| `not_requested` | A collection-only pass did not request analysis or publication |

Do not translate a successful publication process into a claim that a post was created.
`publication-state` from analysis remains the checked **report disposition** (`findings`,
`clean`, `inconclusive`), not the state of the GitHub write.

### Successful analysis

```json
{
  "execution": "success",
  "publication": "disabled",
  "run_url": "https://github.com/example/project/actions/runs/42",
  "artifact_url": "https://github.com/example/project/actions/runs/42/artifacts/7",
  "result": {
    "kind": "analysis",
    "report_file": "report.json",
    "analyzed_sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "expected_platforms": "ubuntu-latest,windows-latest",
    "completed_platforms": "ubuntu-latest,windows-latest",
    "packages": ["library"],
    "observed_head": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
  }
}
```

Use the original JSON from a successful analysis and the same frozen commit and reconciled
platform lists used by publication. The shared evidence parser validates commit, clean-tree
attribution, outcome and census before rendering. The card shows the analyzer's readable
outcome, checked report disposition, judged/in-scope coverage, selected context, actual
engine/target/machine partitions when present, bounded package/platform inventories and a next
action. Findings remain visible with incomplete evidence. Missing platforms never justify a
complete clean disposition. The full report stays in artifacts rather than being duplicated.

`packages` is optional collection-scope disclosure; omission points to collection inputs, not
empty scope. `observed_head` is optional and must come from an actual live-head observation.
Do not fill it from the frozen invocation head merely to claim freshness. Without it, the
card states that live-head freshness was not checked. A differing head establishes staleness,
not a commit distance.

`execution: failure` or `cancelled` can accompany valid completed-platform analysis.
`execution: skipped` cannot. A missing, malformed or mismatched supplied report is an error,
not an empty-scope result. Report diagnostics are displayed as bounded literal text.
The analysis JSON contract is unchanged.

### No analysis

```json
{
  "execution": "success",
  "publication": "not_attempted",
  "run_url": "https://github.com/example/project/actions/runs/42",
  "result": {
    "kind": "unavailable",
    "reason": "empty_scope"
  }
}
```

Reasons are explicit orchestration facts:

| Reason | Source |
| --- | --- |
| `empty_scope` | Preparation selected no benchmarkable packages |
| `fork_pull_request` | The same-repository policy skipped work |
| `no_eligible_commit` | Backfill preparation found no endpoint meeting its age policy |
| `reports_unavailable` | No validated analysis report is available; inspect the job logs |
| `collection_only` | Collection ran without a requested analysis |

Use `reports_unavailable` with the observed failure/cancellation status when work failed.
Do not infer empty scope from a missing file. Successful empty collection or insufficient
history instead uses its successful analysis report when one exists. Its diagnostics point
to actual selected partitions and collection logs, without guessing that compile-only,
smoke-mode or unsupported artifacts caused empty output. Repeated measurements of the same
commit do not create independent history; minimum evidence is not a promised finding.

### Backfill completion

```json
{
  "execution": "success",
  "publication": "not_requested",
  "run_url": "https://github.com/example/project/actions/runs/42",
  "result": {
    "kind": "backfill",
    "from": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "to": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    "expected_platforms": "ubuntu-latest,windows-latest",
    "platforms": [
      {
        "platform": "ubuntu-latest",
        "execution": "success",
        "completion_file": "ubuntu-completion.json"
      },
      {
        "platform": "windows-latest",
        "execution": "cancelled"
      }
    ]
  }
}
```

The core writes a completion file when requested with
`cargo-bench-history backfill FROM TO --completion-output PATH`.
The root companion `action` command forwards optional backfill-only string input
`completion-output` to that flag, resolving it against the measured directory.
The action should choose a new absolute path outside the checkout, retain it before running
the command, and upload an existing completion file even after a stopping benchmark failure.
No new success-shaped action output is emitted on failure; the caller already knows this path.

Completion JSON uses an independent `version: 1` format:

```json
{
  "version": 1,
  "from": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "to": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  "total": 5,
  "counts": { "stored": 1, "skipped": 1, "empty": 1, "failed": 1, "deferred": 1 },
  "stop_reason": "commit_limit",
  "overwrite": false
}
```

Endpoints are frozen full SHAs, and counts partition the entire inclusive range.
`skipped` includes the initial recorded-result scan and write-time duplicates.
`failed` counts per-commit build/benchmark failures, not infrastructure failures.
The stop reason is `range_exhausted`, `commit_limit` or `benchmark_failure`.
Ignored per-commit failures remain counted even when the range exhausts or reaches its bound.
A stopping benchmark failure writes completion evidence but retains the command's nonzero
exit. Final evidence follows successful worktree cleanup and storage-invalidation flushing.

Infrastructure failure or interruption supplies no final accounting. A partially written
file is invalid JSON/evidence, not a completed pass. Treat a missing file as unknown counts;
never scrape logs or substitute zeros. Partial storage writes can remain. A timeout is failed
or cancelled execution, never `commit_limit`. The workflow renderer rejects contradictory
successful execution for a `benchmark_failure` completion.

The action must associate each file with the correct project, workflow run, platform and
executed attempt, retaining the frozen preparation endpoints. Do not reuse a file from an
older successful attempt after a failed retry. The renderer checks endpoints, count
consistency, duplicate/unexpected platform identities and stopping conditions. Missing
platform entries remain explicitly unavailable.

Backfill cards accept only `not_requested` or `disabled` publication. They show final
per-platform counts, stop reasons and continuation guidance, never a benchmark verdict.
No automatic analysis or publication follows. Continue with the same frozen range and
collection settings in the same partition using skip-existing mode; inspect failed or empty
attempts and partial engine writes before retrying. Repeating overwrite restarts at the newest
commit rather than resuming.
