# GitHub automation

This walkthrough starts with a repository that has benchmarks but no GitHub workflows.
It sets up shared storage, collects measurements on the history branch, publishes history
findings to a rolling issue, and compares same-repository pull requests in a rolling PR comment.

The [benchmark-history action](https://github.com/folo-rs/cargo-bench-history-action)
provides individual stages through its **root composite action**. Your workflows own checkout,
benchmark prerequisites, job dependencies, concurrency and artifact uploads. There are no
public reusable `history.yml`, `pr.yml` or `backfill.yml` workflows to call.

## 1. Choose a released action and a benchmark scope

Open the action's
[Releases page](https://github.com/folo-rs/cargo-bench-history-action/releases).
Choose a published version, review its release and pin its full commit SHA. Replace every
`ACTION_RELEASE_SHA` below with that same SHA before enabling the workflows.
If the initial release is not published yet, wait for it; an open action PR, source-mode
canary or monorepo tool release is not evidence that a consumer action release is available.

The action release selects the exact tested tool versions. Its default `install-method:
binstall` downloads published binaries with source-build fallback; `install` builds the
pinned published packages. You do not need to install either benchmark-history binary in
the workflow yourself, and there is no tool-version or token input.

These starters use one Linux runner and a **fixed package scope**:

* Replace `my-library` in each workflow's `BENCHMARK_PACKAGES` with a Cargo package name,
  or a comma-separated list of package names. Both workflows must use the same selection.
* Replace `main` consistently if your history branch has a different name.
* The examples assume stable Rust and Criterion benchmarks with no additional system
  dependencies. Adapt the Rust preparation step to your project's toolchain and install
  any required engine dependencies before collection. For example, Callgrind benchmarks
  need their runner and Valgrind. See [Benchmark engines](concepts/engines.md).
* The action requires Cargo and PowerShell 7.6 or later; verify these on your selected runner.
  The examples use GitHub-hosted `ubuntu-24.04`.

First run your selected benchmarks locally, for example `cargo bench --package my-library`.
They must actually emit results for a supported engine. The action does not create benchmarks.
Commit the configuration and any lockfile changes, and ensure generated build output is
ignored so benchmarking leaves the measured checkout clean.

Fixed scope deliberately benchmarks the chosen packages on every PR, including documentation
changes. This costs more than changed-package selection, but needs no package-detection helper
or empty-scope branch. Collection is scoped; the action's analysis inputs do not include a
package filter. Keep this project's stored history and measurement protocol consistent with
the selected suite rather than mixing unrelated collectors into it.

## 2. Provision shared storage and commit configuration

Both workflows need the **same durable history**. This walkthrough uses Azure Blob storage
so PR analysis can read measurements collected on the history branch. A runner-local directory
alone disappears between jobs. [Storage backends](storage.md) explains local storage if you
instead want to own its persistence, restore and cross-workflow sharing.

[Install the CLI](installation.md) on your administration machine, then follow
[`setup-azure`](commands/setup-azure.md). That chapter walks through reviewing an exported
deployment or provisioning directly, including administrator permissions and optional local
access. Use your GitHub owner, repository name and selected history branch. Record the
deployment's account, container, managed-identity **client ID** and **tenant ID**.

Create `.cargo/bench_history.toml` in your repository root, or use
`cargo bench-history install` to generate a commented starter. Commit these settings after
replacing the example values with your own:

```toml
[project]
id = "example-benchmark-suite"

[storage.azure]
account = "examplehistory"
container = "bench-history"
```

Choose a stable project ID: it selects the shared measurement namespace and the report
identity. History and PR workflows must use the same ID and storage. These examples use the
default configuration location and omit `local-path` so the configured Azure backend is used.

### Configure GitHub permissions and federation

In **Settings → Secrets and variables → Actions → Variables**, add repository variables:

| Variable | Value from `setup-azure` |
| --- | --- |
| `AZURE_CLIENT_ID` | Managed identity's client ID, not its principal/object ID |
| `AZURE_TENANT_ID` | Microsoft Entra tenant ID |

These identifiers are not secrets. The workflows map them into job environment variables.
`id-token: write` lets the tool request short-lived GitHub OIDC tokens and exchange them for
Azure access tokens; the tool handles this directly, without an `azure/login` step.
The runtime flow does not need a subscription ID, storage key, client secret or PAT.

The standard deployment grants the managed identity **Storage Blob Data Contributor at
storage-account scope**, and configures federation for:

* `repo:OWNER/REPOSITORY:ref:refs/heads/main`, using your selected history branch;
* `repo:OWNER/REPOSITORY:pull_request`.

**The PR subject does not distinguish same-repository heads from fork heads.** The PR
workflow below therefore gates the whole job before checkout, benchmarks or credential use.
The action also skips fork-origin PR work, but that is not a reason to remove the job gate.
Never run fork code in a privileged
[`pull_request_target`](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#pull_request_target)
workflow. Only trusted
same-repository PR code belongs in this setup: benchmarks and build scripts execute with
the job's access.

Do not add a job-level GitHub `environment:` to these examples. That changes the OIDC subject
and will not match the generated branch/PR credentials. Job `env:` variables do not do this.
These instructions assume GitHub's standard subject format; customized subject claims also
need matching federation configuration. See GitHub's
[OIDC subject reference](https://docs.github.com/en/actions/reference/security/oidc#example-subject-claims)
and Microsoft's
[managed-identity federation requirements](https://learn.microsoft.com/en-us/entra/workload-id/workload-identity-federation-create-trust-user-assigned-managed-identity).

Enable Actions and Issues for the repository, and ensure organization/repository policies
permit the referenced actions and the requested job permissions. The action uses the job's
ambient short-lived GitHub token for publication. History reporting and failure alerts need
`issues: write`; PR comments need `pull-requests: write`. No credential is passed through an
action input.

## 3. Add the history workflow

Create `.github/workflows/benchmark-history.yml` with the following content, replacing the
release SHA and package selection. It runs on pushes to `main` and permits manual dispatch
on that branch. The concurrency group serializes this project's history publications.

```yaml
name: Benchmark history

on:
  push:
    branches: [main]
  workflow_dispatch:

permissions:
  contents: read

concurrency:
  group: benchmark-history-main
  cancel-in-progress: false

jobs:
  history:
    if: github.ref == 'refs/heads/main'
    runs-on: ubuntu-24.04
    permissions:
      contents: read
      id-token: write
      issues: write
    env:
      AZURE_CLIENT_ID: ${{ vars.AZURE_CLIENT_ID }}
      AZURE_TENANT_ID: ${{ vars.AZURE_TENANT_ID }}
      BENCHMARK_PACKAGES: my-library
    steps:
      - uses: actions/checkout@v4
        with:
          ref: ${{ github.sha }}
          fetch-depth: 0
          persist-credentials: false

      - name: Prepare Rust for the selected benchmarks
        uses: dtolnay/rust-toolchain@stable

      - name: Mark any existing history issue pending
        id: preflight
        uses: folo-rs/cargo-bench-history-action@ACTION_RELEASE_SHA
        with:
          command: publish-issue-preflight
          head: ${{ github.sha }}
          run-id: ${{ github.run_id }}
          run-attempt: ${{ github.run_attempt }}

      - name: Collect this commit
        id: collect
        uses: folo-rs/cargo-bench-history-action@ACTION_RELEASE_SHA
        with:
          command: collect
          packages: ${{ env.BENCHMARK_PACKAGES }}
          all-features: 'false'
          on-existing: skip

      - name: Pass the successful collection's real machine key
        id: keys
        shell: pwsh
        env:
          CBH_MACHINE_KEY: ${{ steps.collect.outputs.machine-key }}
        run: |
          if ([string]::IsNullOrWhiteSpace($env:CBH_MACHINE_KEY)) {
              throw 'Successful collection did not provide a machine key.'
          }
          $root = Join-Path $env:RUNNER_TEMP "benchmark-keys-$([guid]::NewGuid().ToString('N'))"
          $platform = Join-Path $root 'linux-x64'
          New-Item -ItemType Directory -Path $platform | Out-Null
          $env:CBH_MACHINE_KEY | Set-Content -LiteralPath (Join-Path $platform 'machine-key.txt')
          "directory=$root" | Add-Content -LiteralPath $env:GITHUB_OUTPUT

      - name: Analyze the collected history
        id: analyze
        uses: folo-rs/cargo-bench-history-action@ACTION_RELEASE_SHA
        with:
          command: analyze-history
          context: ${{ github.sha }}
          machine-keys: ${{ steps.keys.outputs.directory }}
          expected-platforms: linux-x64
          completed-platforms: linux-x64

      - name: Upload analysis reports
        id: reports
        uses: actions/upload-artifact@v4
        with:
          name: benchmark-history-${{ github.run_id }}-${{ github.run_attempt }}
          path: |
            ${{ steps.analyze.outputs.report-markdown }}
            ${{ steps.analyze.outputs.report-json }}
            ${{ steps.analyze.outputs.report-summary }}
          if-no-files-found: error

      - name: Publish the validated history state
        uses: folo-rs/cargo-bench-history-action@ACTION_RELEASE_SHA
        with:
          command: publish-issue-${{ steps.analyze.outputs.publication-state }}
          body-file: ${{ steps.analyze.outputs.report-summary }}
          report-file: ${{ steps.analyze.outputs.report-json }}
          analyzed-sha: ${{ github.sha }}
          artifact-url: ${{ steps.reports.outputs.artifact-url }}
          expected-platforms: linux-x64
          completed-platforms: linux-x64
          run-id: ${{ github.run_id }}
          run-attempt: ${{ github.run_attempt }}

      - name: Retire this run's unfinished history state
        if: ${{ (failure() || cancelled()) && steps.preflight.outcome == 'success' }}
        uses: folo-rs/cargo-bench-history-action@ACTION_RELEASE_SHA
        with:
          command: publish-issue-failed
          head: ${{ github.sha }}
          run-id: ${{ github.run_id }}
          run-attempt: ${{ github.run_attempt }}
          conclusion: ${{ job.status }}

      - name: Publish a workflow-failure alert
        if: ${{ failure() || cancelled() }}
        uses: folo-rs/cargo-bench-history-action@ACTION_RELEASE_SHA
        with:
          command: alert
```

Steps use GitHub's default success gating. There is no `continue-on-error`: a failed collect
cannot reach machine-key handoff, analysis or successful-state publication. Because collection
and analysis are sequential in this one job, reaching analysis establishes that `linux-x64`
completed in this attempt. `machine-keys` is a **directory**, containing the real collection
output in `<platform>/machine-key.txt`, not a runner label or an invented fingerprint.
It is outside the checkout so preparing it does not dirty the measured source.

`on-existing: skip` supports manual reruns without replacing stored measurements. It still
runs the benchmarks, so failures remain visible. The starters use default Cargo features
explicitly; keep any feature, target, runner and repetition changes consistent across history
and PR collection. See [Measurement stability](concepts/stability.md).

### Verify the first history run

Commit the configuration and workflow to your history branch. In **Actions → Benchmark
history**, inspect the push-triggered run. Check that collection ran the intended benchmarks,
stored measurements under the expected project and emitted a machine key. Check that analysis
and the report upload succeeded; download the run's `benchmark-history-…` artifact and read
the full Markdown report and JSON coverage census.

Then use **Run workflow**, select `main`, and dispatch again. The file must be present on the
default branch for GitHub's manual-dispatch UI. The job deliberately skips a dispatch on any
other branch, which would not match the configured history federation subject.

**An initial successful run need not create an issue.** Only findings create the rolling
history issue. Initial clean and no-data publication are logged no-ops when no issue is open.
When an issue does exist, later clean results annotate it as all-clear but leave it **open**;
no-data cannot clear it. Preflight and failed states update only appropriate existing state.
Report artifacts and workflow logs are therefore the first-run success evidence, not the
presence of an issue.
When findings do appear, look in **Issues** for the rolling
`Benchmark history findings for …` issue.

## 4. Let the baseline warm up

A single history measurement has no earlier baseline. Repeating collection at the same commit
does not add distinct historical points. Let the history workflow collect across commits before
expecting useful comparisons, or seed a bounded range with [backfill](commands/backfill.md).
For workflow-based seeding, the same root action's `command: backfill` accepts `from` and `to`
commit inputs, the collection package/feature inputs and `on-existing: skip`. Run it in a
manually dispatched history-branch job with the same storage, runner and benchmark prerequisites.
Backfill only collects; run the history workflow afterward to analyze and publish.

Hosted runners can have different hardware fingerprints even with the same runner label.
Baselines must match the actual machine key and the other
[comparability dimensions](concepts/comparability.md). A new runner type, engine or toolchain
can therefore need its own warmup. Do not manufacture a stable key to join incompatible data.
Missing or insufficient history is a successful **no-data publication**, not a clean verdict
and not an execution failure.

## 5. Add the pull-request workflow

Create `.github/workflows/benchmark-pr.yml`. Use the same action release, package selection,
configuration and benchmark prerequisites as history collection.

The job accepts only same-repository PRs targeting the selected history branch. Checkout uses
the event's **real head SHA**, not GitHub's synthetic PR merge commit. Analysis uses the
event's frozen base SHA, with full history fetched so both commits and their merge base are
available. A newer push creates another run; publication checks live-head freshness rather
than relabeling an older result as current.

```yaml
name: Benchmark pull request

on:
  pull_request:
    branches: [main]
    types: [opened, synchronize, reopened]

permissions:
  contents: read

concurrency:
  group: benchmark-pr-${{ github.event.pull_request.number }}
  cancel-in-progress: false

jobs:
  benchmark:
    if: github.event.pull_request.head.repo.full_name == github.repository
    runs-on: ubuntu-24.04
    permissions:
      contents: read
      id-token: write
      pull-requests: write
      issues: write
    env:
      AZURE_CLIENT_ID: ${{ vars.AZURE_CLIENT_ID }}
      AZURE_TENANT_ID: ${{ vars.AZURE_TENANT_ID }}
      BENCHMARK_PACKAGES: my-library
    steps:
      - uses: actions/checkout@v4
        with:
          ref: ${{ github.event.pull_request.head.sha }}
          fetch-depth: 0
          persist-credentials: false

      - name: Prepare Rust for the selected benchmarks
        uses: dtolnay/rust-toolchain@stable

      - name: Mark this PR's benchmark report pending
        id: preflight
        uses: folo-rs/cargo-bench-history-action@ACTION_RELEASE_SHA
        with:
          command: publish-comment-preflight
          packages: ${{ env.BENCHMARK_PACKAGES }}
          pr-number: ${{ github.event.pull_request.number }}
          head: ${{ github.event.pull_request.head.sha }}
          run-id: ${{ github.run_id }}
          run-attempt: ${{ github.run_attempt }}

      - name: Collect the PR head
        id: collect
        uses: folo-rs/cargo-bench-history-action@ACTION_RELEASE_SHA
        with:
          command: collect
          packages: ${{ env.BENCHMARK_PACKAGES }}
          all-features: 'false'
          on-existing: skip

      - name: Pass the successful collection's real machine key
        id: keys
        shell: pwsh
        env:
          CBH_MACHINE_KEY: ${{ steps.collect.outputs.machine-key }}
        run: |
          if ([string]::IsNullOrWhiteSpace($env:CBH_MACHINE_KEY)) {
              throw 'Successful collection did not provide a machine key.'
          }
          $root = Join-Path $env:RUNNER_TEMP "benchmark-keys-$([guid]::NewGuid().ToString('N'))"
          $platform = Join-Path $root 'linux-x64'
          New-Item -ItemType Directory -Path $platform | Out-Null
          $env:CBH_MACHINE_KEY | Set-Content -LiteralPath (Join-Path $platform 'machine-key.txt')
          "directory=$root" | Add-Content -LiteralPath $env:GITHUB_OUTPUT

      - name: Analyze this head against its base
        id: analyze
        uses: folo-rs/cargo-bench-history-action@ACTION_RELEASE_SHA
        with:
          command: analyze-pr
          context: ${{ github.event.pull_request.head.sha }}
          base: ${{ github.event.pull_request.base.sha }}
          machine-keys: ${{ steps.keys.outputs.directory }}
          expected-platforms: linux-x64
          completed-platforms: linux-x64

      - name: Upload analysis reports
        id: reports
        uses: actions/upload-artifact@v4
        with:
          name: benchmark-pr-${{ github.run_id }}-${{ github.run_attempt }}
          path: |
            ${{ steps.analyze.outputs.report-markdown }}
            ${{ steps.analyze.outputs.report-json }}
            ${{ steps.analyze.outputs.report-summary }}
          if-no-files-found: error

      - name: Publish the validated PR state
        uses: folo-rs/cargo-bench-history-action@ACTION_RELEASE_SHA
        with:
          command: publish-comment-${{ steps.analyze.outputs.publication-state }}
          packages: ${{ env.BENCHMARK_PACKAGES }}
          pr-number: ${{ github.event.pull_request.number }}
          body-file: ${{ steps.analyze.outputs.report-summary }}
          report-file: ${{ steps.analyze.outputs.report-json }}
          analyzed-sha: ${{ github.event.pull_request.head.sha }}
          artifact-url: ${{ steps.reports.outputs.artifact-url }}
          expected-platforms: linux-x64
          completed-platforms: linux-x64
          run-id: ${{ github.run_id }}
          run-attempt: ${{ github.run_attempt }}

      - name: Retire this run's unfinished PR state
        if: ${{ (failure() || cancelled()) && steps.preflight.outcome == 'success' }}
        uses: folo-rs/cargo-bench-history-action@ACTION_RELEASE_SHA
        with:
          command: publish-comment-failed
          pr-number: ${{ github.event.pull_request.number }}
          head: ${{ github.event.pull_request.head.sha }}
          run-id: ${{ github.run_id }}
          run-attempt: ${{ github.run_attempt }}
          conclusion: ${{ job.status }}

      - name: Publish a workflow-failure alert
        if: ${{ failure() || cancelled() }}
        uses: folo-rs/cargo-bench-history-action@ACTION_RELEASE_SHA
        with:
          command: alert
```

Open a PR from a branch **in this repository**. Its run should create a pending comment,
collect the head, upload the report artifact and replace the comment with findings, clean or
no-data. Confirm the report names the intended head and base, and inspect the coverage rather
than interpreting a quiet report as an all-clear. Push another commit to verify the same
comment is maintained. Use **Re-run all jobs** to retry a failed run; the frozen event commits
remain the same. A fork PR should skip this job and receive no credentialed benchmark run.
The PR workflow grants `issues: write` only for its separate workflow-failure alert.

## 6. Read the result and handle failures

The analysis action does not publish or upload anything. It returns:

| Output | How these workflows use it |
| --- | --- |
| `outcome` | Scalar analysis verdict, not a path; see [Analysis outcomes](appendix/reporting.md#analysis-outcomes) |
| `publication-state` | Validated `findings`, `clean` or `no-data`, used to select the publication command |
| `partial-platform-coverage` | Whether expected collection platforms are missing |
| `report-markdown`, `report-json`, `report-summary` | Job-local paths uploaded together; summary and JSON also go to publication |

Keep the summary and JSON from the **same successful analysis** paired. The artifact-upload
step returns the URL passed to publication; paths are not cross-job artifact identifiers.
No example invokes the companion binary directly or assumes it was added to `PATH`.

Findings are advisory and do not fail the job. Findings still publish when some analyzed series
could not be judged, with coverage qualification. A complete all-clear needs both a fully
judged analysis and complete expected-platform evidence. `notable: false` alone is not enough:
insufficient baseline, partially judged series or no series in scope can require no-data.
The public `publication-state` output performs this selection without parsing Markdown or
calling a private inspection command.

Failure is different. Failed collection, analysis, upload or publication leaves the workflow
failed. The terminal step attempts to retire only this run attempt's unfinished preflight
state; it cannot replace a valid report with a false analysis verdict. The alert step attempts
to create a separate issue for the failed workflow run. Later successful runs do not close
that `Benchmark history workflow failed for …` alert. If installation or required configuration
fails, the companion may be unavailable
and neither notification can be guaranteed; the Actions run remains the diagnostic source.
Cancellation can also interrupt cleanup.

When diagnosing a first run:

* For authentication failures, check repository variables, `id-token: write`, the exact
  branch/PR subject and the managed identity's storage-account data role. Do not fix a subject
  mismatch by broadening trust to untrusted PR execution.
* For empty/no-data reports, confirm that supported benchmarks emitted results, the selected
  packages are correct, and enough matching baseline history exists.
* For report rejection, check that the checkout remained clean, full Git history is available,
  and all evidence comes from the same analyzed commit and invocation.
* For a missing issue or comment, distinguish an intentional history no-op or stale-result
  preservation from a failed publication step. Review the action's explanatory diagnostics.

## Extending beyond the starter

The workflow graph remains yours. Keep publication serialized per project and destination
when adding triggers or another workflow. Commands reject inputs that do not apply to them;
consult `action.yml` and the README for your selected release in the
[action repository](https://github.com/folo-rs/cargo-bench-history-action) rather than passing
collection options to analysis.

If you add changed-package selection, an empty selection must explicitly route to
`publish-comment-no-data` with `empty-scope: 'true'` and the frozen head/PR/run identity.
Do not pass an empty `packages` value to collect: at the root action boundary it means the
whole workspace. Missing collection evidence is not empty scope.

If you split collection and analysis into multiple jobs or a platform matrix, the sequential
proof used above no longer suffices. Use the existing **collection-receipt protocol**:
successful collection binds repository, project, run, attempt, frozen head, platform and
machine key in a receipt, and analysis preparation reconciles those receipts against actual
job attempts. A failed retry excludes older success for that platform; a platform not retried
may retain its earlier successful receipt. At least one validated collection must succeed.
Pass the intended platforms as `expected-platforms`, only the validated successful subset as
`completed-platforms`, and only those platforms' real keys to analysis. Never reconstruct
completion from fingerprints or artifact presence alone.

That orchestration is not supplied by a public reusable workflow or automatically assembled
by the root action. Keep the single-job starter unless you are prepared to integrate the
receipt-based job graph and artifact handoff. Partial collection must remain visible in
reports, and a failed leg must not become a complete all-clear merely because another leg
produced a clean analysis.
