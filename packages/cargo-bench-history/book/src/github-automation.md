# GitHub automation

Use the prebuilt
[benchmark-history workflows](https://github.com/folo-rs/cargo-bench-history-action)
to collect benchmark history and turn it into GitHub reports:

* The **history workflow** collects measurements on pushes to `main`, analyzes the
  accumulated history and maintains a rolling issue for findings.
* The **pull-request workflow** considers all workspace packages and automatically
  selects the benchmarks affected by a PR. It compares their results against the
  PR's base and maintains a report comment.
* The **backfill workflow** fills gaps across a commit range, skipping measurements
  already stored for the runner's platform and machine key. It only stores data;
  it does not analyze or publish reports.

The workflows handle checkout, tool installation and collection; history and PR also
handle analysis and report publication. Their default collection platforms are Linux
and Windows. Apple Silicon macOS is also supported: add
`platforms: ubuntu-latest,windows-latest,macos-latest` to
each caller's `with:` block to include it. Your benchmarks and their dependencies
must support the selected platforms. You add the caller files and connect shared storage.

First, run `cargo bench --workspace --all-features` to check your benchmarks. If you
are adding benchmarks, start with a [supported benchmark engine](concepts/engines.md).

## 1. Set up Azure storage

Follow [`setup-azure`](commands/setup-azure.md) to provision storage and a workflow
identity for your repository, using `main` as the history branch. Keep its printed
storage account, container, managed identity client ID and Azure tenant ID for the
following steps. For other storage choices, see [Storage backends](storage.md).

Create `.cargo/bench_history.toml` in your repository root. Set `project.id` to a name
for your workspace's benchmark history, and copy the account and container from
the setup output:

```toml
[project]
id = "my-workspace"

[storage.azure]
account = "myhistoryaccount"
container = "bench-history"
```

In **Settings → Secrets and variables → Actions → Variables**, create these
repository variables from the same output:

| Variable | Value |
| --- | --- |
| `AZURE_CLIENT_ID` | Managed identity client ID |
| `AZURE_TENANT_ID` | Azure tenant ID |

These are identifiers, not secrets. The workflows use short-lived identity tokens
to authenticate; you do not need a stored credential or an `azure/login` step.

## 2. Add the history workflow

Create `.github/workflows/benchmark-history.yml`:

```yaml
name: Benchmark history

on:
  push:
    branches: [main]
  workflow_dispatch:

jobs:
  history:
    if: github.ref == 'refs/heads/main'
    permissions:
      contents: read
      actions: read
      id-token: write
      issues: write
    uses: folo-rs/cargo-bench-history-action/.github/workflows/history.yml@v1
    with:
      azure-client-id: ${{ vars.AZURE_CLIENT_ID }}
      azure-tenant-id: ${{ vars.AZURE_TENANT_ID }}
```

The workflow collects your workspace's benchmarks and stores the measurements in
Azure. The permissions let it read the repository and run artifacts, authenticate
to Azure, and publish findings or workflow-failure alerts as issues.

Commit the configuration and workflow to `main`. Open **Actions → Benchmark history**
to watch the first run. When it finishes, download the `bench-history-report-…`
artifact from the run's **Artifacts** section. It contains the full Markdown report,
JSON report and condensed summary.

To run it manually, select **Run workflow** on that Actions page and choose `main`.

## 3. Add the pull-request workflow

Create `.github/workflows/benchmark-pr.yml`:

```yaml
name: Benchmark pull request

on:
  pull_request:
    branches: [main]
    types: [opened, synchronize, reopened, closed]

jobs:
  benchmark:
    if: github.event.pull_request.head.repo.full_name == github.repository
    permissions:
      contents: read
      actions: read
      id-token: write
      pull-requests: write
    uses: folo-rs/cargo-bench-history-action/.github/workflows/pr.yml@v1
    with:
      azure-client-id: ${{ vars.AZURE_CLIENT_ID }}
      azure-tenant-id: ${{ vars.AZURE_TENANT_ID }}
```

Commit this file to `main`, then open a PR that changes benchmarked code, using a
branch in your repository. The workflow posts a pending comment, runs the benchmarks,
and updates that comment with the result and a link to the `pr-bench-history-report-…`
artifact. Pushing another commit updates the same comment; closing the PR cancels
any outstanding benchmark run.

The PR workflow skips forks. With GitHub's default settings, GitHub's servers remove
`id-token: write` from fork PR jobs, so those jobs cannot request the identity token
needed to access Azure. Editing the workflow or approving its run does not restore
that permission. Workflows running in the fork itself use the fork's identity, which
does not match your Azure trust. Azure's PR trust rule names your repository, not the
PR's source repository: GitHub's permission restriction, not that Azure rule or a
workflow condition, prevents fork access.

## 4. Fill gaps with backfill

Backfill is useful when hosted runners leave sparse per-machine history or when you
want to seed earlier commits. Create `.github/workflows/benchmark-backfill.yml`:

```yaml
name: Benchmark backfill

on:
  schedule:
    - cron: '0 2 * * *'
  workflow_dispatch:
    inputs:
      to:
        description: Optional range-end override
        required: false
        type: string

jobs:
  backfill:
    if: github.ref == 'refs/heads/main'
    permissions:
      contents: read
      id-token: write
    uses: folo-rs/cargo-bench-history-action/.github/workflows/backfill.yml@v1
    with:
      azure-client-id: ${{ vars.AZURE_CLIENT_ID }}
      azure-tenant-id: ${{ vars.AZURE_TENANT_ID }}
      lookback: 14 days
      minimum-age: 24 hours
      to: ${{ inputs.to }}
```

This example runs nightly and permits manual dispatch on `main`, matching the Azure
branch trust configured above. Shared preparation chooses the newest first-parent
commit at least `minimum-age` old, then finds its oldest first-parent ancestor within
`lookback` of preparation time. The optional `to` input overrides the age-based end
selection; the lookback still runs from preparation time, not from the selected end.
If that history is older than the horizon, backfill measures just the selected end.
If no automatic end is eligible, the workflow explains the no-op and starts no
benchmark jobs.

Both duration inputs are required for rolling selection. They accept friendly spans
such as `24 hours` or `24 hours ago`, and ISO spans such as `P14D`; their magnitudes are
resolved from one UTC clock snapshot, including calendar units. `lookback` must be
nonzero; `minimum-age` may be zero.

To select an explicit range instead, replace the duration inputs with `from` and
`to`, each a nonblank, single-line commit reference. Do not combine `from` with
`lookback` or `minimum-age`. The start must be a first-parent ancestor of the end.
Preparation freezes the range to full commit SHAs and execution checks out its end.
The core tool visits the inclusive range newest-first, using each historical workspace
rather than filtering against benchmarks present at the invocation head.

Already-stored commits are skipped in the current platform/machine partition, so you
can resume an interrupted run by dispatching the same range. Each platform job has a
six-hour ceiling and runs independently of other platforms' failures. Backfill runs
queue without deduplicating or cancelling earlier invocations; project/platform
queues also serialize config paths that identify the same storage project.

Set `max-commits: '1'` to attempt at most one missing commit per platform job, or choose
another positive integer. Omitted or empty input leaves replay unlimited. The limit applies
after skipping recorded commits in the current partition and does not shorten the inclusive
range. Failed, empty and write-time duplicate attempts count, while pre-check skips do not.
Each attempt completes all repetitions and engine storage, and a bounded pass exits normally
with a deferred-work count after cleanup. This bounds work, not elapsed time.

Build or benchmark failures stop by default. Set `ignore-errors: true` to continue
past individual failing commits; infrastructure errors still fail. Independently,
`best-effort: true` tolerates ordinary platform-job failures but does not turn a
hosted-runner timeout into a successful bounded pass. Both default to false. Best effort
can also hide credential or storage failures, so use it only when that is an acceptable
policy for your caller.

Backfill accepts the same measurement and installation settings as the other workflows:
`platforms`, `working-directory`, `config`, `exclude`, `bench`, `best-of`, `all-features`,
`no-default-features`, `features`, `rustflags`, `install-method` and `source-path`. Match your history
caller's feature and repetition settings so measurements remain comparable. There is
no package allowlist or overwrite option in the reusable workflow.

Configuration, source-installed tools and the optional
`.github/actions/bench-history-setup/action.yml` hook come from the invocation checkout,
not the historical commits. The shared workflow invokes that fixed hook before
benchmarking; no setup-path input is needed. A nested `working-directory` selects the
same relative project directory in each historical worktree.

`rustflags` appends rustc arguments using Cargo's whitespace-separated `RUSTFLAGS`
syntax, without shell quoting. Existing encoded Cargo arguments take precedence over
ambient `RUSTFLAGS` and keep their argument boundaries. An empty input preserves the
environment. Set the same compiler options for history, PR and backfill; custom setup
hooks need only prepare repository-specific build prerequisites.

The reusable workflow calculates the range; your caller supplies only parameters
and a schedule. It emits no receipts, report
artifacts or public outputs, and has no analysis or publication sink; later history
runs analyze the stored measurements. See [backfill](commands/backfill.md) for core
range, storage and measurement semantics.

## 5. Read the reports

Findings describe benchmark changes and are advisory: they do not fail the workflow.
Read the report's coverage information alongside its findings. A complete all-clear
requires enough comparable data and successful collection on every expected platform.

The first runs often have too little history to compare against. Their publication
state is **inconclusive**, not clean. An inconclusive report can also mean that only
some benchmarks could be judged or that collection coverage was incomplete. The
report's [analysis outcome](appendix/reporting.md#analysis-outcomes) and coverage
explain the cause; inconclusive publication does not mean analysis failed.
Let the history workflow collect measurements as new commits reach `main`, or
seed earlier commits with [backfill](commands/backfill.md).

Only findings create the rolling `Benchmark history findings for …` issue.
An initial clean or inconclusive history report therefore leaves no issue; inspect
its artifact instead. Once a findings issue exists, a later complete clean report
marks it all-clear but leaves it open. Inconclusive results cannot clear it.
PRs receive a comment even for clean or inconclusive results. If no benchmark
packages were affected, the comment explains that nothing was benchmarked.

Execution failures are different: inspect the failed Actions run and its diagnostics.
For a failed history run, the workflow also attempts to publish a
`Benchmark history workflow failed for …` alert issue. Later successful runs do not
close these alerts.
