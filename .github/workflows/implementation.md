# GitHub workflows implementation

This guide maps the workflow design to the repository tools that implement it. User-visible
CI behavior and design tenets are in [design.md](design.md); command flags and step details stay
with the commands and workflow jobs.

## Workflow events and orchestration

GitHub cron times are UTC. Local App schedules are configured separately using
the installed App's scheduling controls.

| Workflow | What starts it | Responsibility |
|---|---|---|
| `deep-validation.yml` / **Deep validation** | Daily at 02:41 UTC, or no-input manual dispatch on `main`. | Execute full standard and deep main-branch validation, preserve diagnostics and report failures within one run. |
| `standard-validation.yml` / **Standard validation** | Push to `main`, PR opened/synchronized/reopened/ready for review, or a reusable call from Deep validation. | Shallow checks feeding the single required `required-checks` result. |
| `merge-queue-validation.yml` / **Merge queue validation** | Merge-group checks requested for `main`. | Full-workspace dev Clippy, formatting and version readiness feeding `required-checks`. |
| `pr-bench-history.yml` / **PR Benchmark history** | PR opened/synchronized/reopened; closed events cancel without collecting. | Production-backed PR collection and combined analysis/publication for same-repository PRs. |

```text
Deep validation on main: plan -> standard + deep checks -> report failures -> run report issue

Local App triage -> run report -> existing or new problem issues
Local App repair -> claimed problem issue -> PR -> human review and merge

PR / main push -> Standard validation -> required-checks
Merge queue -> Merge queue validation -> required-checks
```

Hosted reporting does not invoke AI or wait for triage. App automations discover
work through ordinary GitHub issues. Closing a triaged run report is independent
of resolving its linked problems. A merged repair closes its issue through the
normal PR relationship, not a main-push confirmation workflow.

Standard validation and its close companion share the `standard-validation-`
ref-specific concurrency group. The merge-blocking job/check name and ruleset
target are exactly `required-checks`.

## Benchmark workflow artifacts

Collection and backfill use the fixed `bench-history-setup` local action. Folo's wrapper
selects the shared setup environment with Valgrind enabled; it contains only that configuration.
This keeps repository-specific preparation separate from tool installation and GitHub posting.

Benchmark automation has preparation, collection, combined analysis/publication and
independently scheduled lifecycle work.
Preparation uses the standard `setup-environment` action and its shared development-tool caches.
It builds the Linux companion in Cargo's ordinary target directory, resolves the executable
from Cargo's build output and archives its executable permissions. The combined job and
lifecycle jobs reuse that run-scoped archive rather than independently rebuilding the companion.
Notification depends on this executable and has no independent publisher. A failure to
prepare or obtain the companion keeps the workflow visibly failed rather than claiming
notification success; issue alerting is not guaranteed while the executable is unavailable.

The companion turns the configured platform CSV into the matrix and collection job prefix.
Collection jobs use `cbh-collect:<instance>:<platform>` identities. A successful leg produces
`receipt.json` with its repository, instance, workflow run/attempt, frozen head, platform and
machine key. Collection artifacts contain only that receipt; measurements remain in the
configured store. Artifact names are stable per platform within a run and overwritten on
successful reruns.

Analysis downloads through the REST run-artifacts endpoint so surviving older-attempt
artifacts remain visible. Rust reconciles receipts with each platform's latest job attempt,
then writes the selected machine keys for ordinary configured-store analysis. It
rejects missing, conflicting or mismatched evidence instead of silently narrowing success.
Collection artifacts are outside the persisted history cache.
Artifact downloads pass the ambient GitHub token, repository and run ID explicitly with
Actions-read permission. Fork-origin PR runs have a read-only base-repository token capable
of artifact reads; the same-repository workflow gate is independent of that capability.
The temporary machine-key file stays outside the uploaded collection root: its value is
captured in the receipt rather than uploaded as a separate file.

Automation and measured source are separate for PR runs. The workflow's merge checkout
supplies current helpers, tool builds and configuration. The full real-head checkout under
`benchmark-source` supplies Cargo scope, benchmark execution and git topology. Collection
passes its repository and the automation configuration explicitly; analysis passes that
repository and the frozen event head/base. This preserves real commit attribution without
requiring every open PR head to contain new automation files.
The source-built collector inherits the automation-selected toolchain while benchmarking the
frozen head; measurement provenance records that compiler.

The analysis bundle always contains the tool's full Markdown, JSON and summary. The companion
projects validated JSON into `outcome`, `notable`, `can-clear` and `publication-state` outputs.
The history-only `can-clear` output describes eligibility for the all-clear presentation;
`clean` is a publication-state value. The
[companion command contract](../../packages/cargo-bench-history-github/docs/implementation.md#command-and-artifact-contract)
owns these outputs. The same job uploads the reports, then publishes using those local files
and the artifact link.
Main issue writers share an instance concurrency group, and PR comment writers share a project/PR
group. Issue lookup uses project-qualified title search followed by a current read by number;
PR comment lookup remains marker-based. Body markers carry commit and state; outputs outside
the companion's defined formats are not adopted. Titles, advisory wording and book links
come from the companion's message catalogue rather than workflow parameters.

Both sinks use `publish-<sink>-<state>` commands. A successful report selects findings, clean
or no-data from validated evidence; scope preflight supplies explicit empty scope when no
analysis is needed. Preflight and failed-state commands share run/attempt/head ownership,
and failed publication never replaces a completed report. Issue no-data/failed annotations
preserve the existing investigation rather than creating a status-only issue. The separate
`alert` uses project/run identity, includes closed issues in deduplication and has no
successful-run resolution job. Partial collection can publish qualified findings and alert
on failed jobs without publishing failed status over those findings.

Preflight publishes its actual run-attempt as a job output. A partial rerun can reuse that
successful job, so terminal commands use the preserved owner attempt rather than assuming
the current execution created the pending marker. Successful reports use the current attempt.
The combined analysis/publication job waits for preflight to finish but can still publish
after a failed preflight; a delayed start notice cannot arrive after its own result.

Azure configuration uses `AZURE_PROD_CLIENT_ID` for every production-history operation.
Empty PR scope needs no Azure access. The shared identity has contributor access and branch/PR
federation; PR collection and analysis receive OIDC permission and the analysis/publication job
also receives its GitHub posting scope. PR analysis restores but never saves the Actions
history cache. The tool re-lists configured storage and reads newly stored objects on a
cache miss; topology selection excludes unrelated branch commits from trunk analysis.

Manual pruning and ordinary backfill cover the supported data-maintenance path. Benchmark
workflows expose no targeted historical recollection. Their triggers exclude `merge_group`
and enqueue/dequeue activity because the workflows are advisory.

### Shared action migration

The job graphs and CI-only `gh-*` recipes are intermediate wiring for issue #284.
Their migration targets in `folo-rs/cargo-bench-history-action` are:

| Folo workflow | Shared reusable workflow |
| --- | --- |
| `bench-history.yml` | `.github/workflows/history.yml` |
| `pr-bench-history.yml` | `.github/workflows/pr.yml` |
| `bench-history-backfill.yml` | `.github/workflows/backfill.yml` |

Once those reusable workflows are published, they own installation, collection and analysis
orchestration, receipt/artifact handoff, publication lifecycles and job coordination.
Their root composite action supplies the individual tool commands. Folo retains its triggers,
repository configuration, caller permissions/inputs and the fixed `bench-history-setup` hook,
using source installation to exercise the monorepo tools.
The CI-only recipes, companion-archive builder and helpers without other callers can then be
removed. The initial root-action release alone does not provide the reusable-workflow layer.
Manual Azure provisioning remains a separate maintainer operation through `setup-azure`.

## Standard validation structure

`standard-validation.yml` groups related checks into shared environments, with sequential
steps that retain independent findings after a failure. Independent jobs and platform legs
retain parallel execution with matrix fail-fast disabled. Check-step conditions use
`!cancelled()` and successful setup outcomes rather than GitHub's implicit `success()` gate.
They preserve the original scope conditions and any check-specific prerequisites. Failed steps
keep their failure status, so the combined job fails without a separate verdict step or
`continue-on-error`. Setup and other genuine prerequisite chains still stop when an input fails.
The `prepare` job publishes the affected-package set and the independent path plan, supplemented by native-helper package
impact for script integration tests. Release validation remains unconditional and independent
of preparation, with separately reported steps in one environment.

The `clippy-dev-docs` matrix shares setup for dev-profile Clippy, documentation and
minimum-dependency compilation; its Ubuntu leg first checks workspace formatting.
Documentation steps build all-feature and default-feature documentation and run doctests.
They use the same Linux, macOS and Windows matrix as Clippy and minimum-dependency compilation,
including on pull requests. `check-frozen` runs last because it
rewrites the manifests and lockfile, so later checks cannot accidentally use frozen inputs.
Each platform proceeds independently rather than waiting for other platforms' Clippy results.
The `test-x64` matrix shares its Linux/Windows environment across coverage-instrumented tests,
benchmark smoke tests and external-type checks. Test or upload failures do not suppress
benchmark or external-type checks. Coverage reporting consumes successful measurement;
uploading consumes the generated report.

Only pull requests use affected-package and tooling-input selection. Pushes to `main` and
scheduled/manual main runs use the full set without invoking delta.

### Azure emulator coverage

The Azurite coverage job runs both the CLI integration suite and the storage partition's
adapter tests with a required emulator. Selecting only the CLI package would exercise
production storage through commands but omit the adapter unit tests, which cover additional
network paths. The combined selection contributes their coverage to the same Azure upload.

### Real-Azure authentication

`test-azure` runs `just test-azure` with the developer credential and then with the application's
self-minting GitHub OIDC credential. Setup, compilation and `azure/login` are shared.
Both credential passes require successful login, but either pass can report failures
independently of the other.
An empty step-local `AZURE_CLIENT_ID` selects the developer credential; the self-minting
step sets it to `AZURE_TEST_CLIENT_ID`. The latter ignores the Azure CLI session for application
storage access, while test-container cleanup still uses that session.

The job retains the same-repository PR gate required by the test identity. Scheduled main
runs use its existing main-branch federated subject; queue validation has no Azure job.
Each scenario creates its own container, and an always-run cleanup sweep collects older leaks.
The age guard preserves recently written containers used by concurrent workflow runs.

### Non-Cargo change planning

The `prepare` job runs `scripts/build/ValidationPlan.psm1` with Git and preinstalled
PowerShell before setting up the development environment for Cargo delta. Rust is impractical
at this boundary because setup has not run; the path planner also supports callers without
a prepared Rust environment. Both planners share one full-history checkout and runner.

The planner reads immutable event SHAs from a full-history checkout. Pull requests compare
their head with its merge base against the event's base SHA, covering all PR commits without
including unrelated base-branch changes. Git emits NUL-delimited paths with rename detection
disabled, so a move contributes both its removed path and its added path without filename
quoting ambiguity.
Unavailable revisions fail. Main pushes and scheduled/manual main runs explicitly select
the full suite without reading change-set revisions.

Script directories are coarse test domains. The module declares recipe ownership and
cross-domain consumers, and defaults unfamiliar script/recipe locations to the full suite.
Setup, shared utility, planner and fan-in changes select every tooling check. Fixtures select
their owning tests; workflow changes also select Pester dependency-relationship and helper tests. Analyzer
configuration and script files select static analysis independently of the Pester scope.

After Cargo delta, the same job combines the path-selected domains with affected native helpers used by
script integration tests. Dependency impact comes from Cargo delta rather than treating
every lockfile change as a full-script-suite trip wire. Live manifest changes also select the
scheduled tests that read workspace metadata. The resulting domain array is explicit even
when empty. `test-scripts` runs that union once; `just test-scripts "book release"` is the local
equivalent, while an omitted argument retains full discovery. Unknown domains or explicit
directories containing no tests fail rather than producing a successful empty run.

The `test-scripts` job also runs static analysis in the same environment. It is selected when
either analysis or tests are needed, while each step keeps its own path/domain condition.
Analysis runs first; its failure does not suppress selected tests. Diagnostics are uploaded
even after a failure.

Recipe files follow their automation responsibility: benchmark history and release commands
have separate imports, while setup installers live beside the setup module. Workflow
entrypoints stay in GitHub's required location. Co-location reduces selection coupling but
does not replace declared dependencies.

### Workflow lint environment

`setup-workflow-lint` restores the same portable actionlint/ShellCheck cache as
`setup-environment` and invokes the same pinned, checksum-verifying installers under
`scripts/setup`. It needs neither Rust toolchain setup nor system package installation.
The workflow invokes `actionlint -color` directly, matching `just validate-workflows` without
installing Just solely to dispatch that command. Local full setup continues to install these
same binaries. Keep the command and cache keys aligned when editing these entry points.

The push-to-main and PR benchmark collection queues use GitHub's
[`queue: max` concurrency setting](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/control-workflow-concurrency#example-queueing-multiple-pending-runs)
so waiting commits and PRs are retained instead of replacing one another. The pinned actionlint does not
recognize this key ([upstream issue](https://github.com/rhysd/actionlint/issues/657)).
`.github/actionlint.yaml` excludes only that exact diagnostic for `bench-history.yml` and `pr-bench-history.yml`;
other concurrency diagnostics and other workflows remain checked.

## Merge queue validation

The queue workflow is independent of Standard validation and has no preparation/delta job.
Its Clippy matrix runs `just clippy dev` with no package selector on the same platforms as
standard dev Clippy. The Ubuntu leg first runs `just format-check`, sharing setup.
Clippy still runs after a formatting failure when setup succeeded.
A separate full-history job runs only `just validate-versions` with
`RELEASE_PLAN_BASE` set to the event's immutable `merge_group.base_sha`. This prevents a
moving `origin/main` from changing the candidate's release baseline.

The queue does not inherit minimum-dependency, binstall or SemVer steps from the similarly
named standard jobs. Its fan-in names every dependency as must-succeed and retains the
literal `required-checks` check name. Queue-ref concurrency is independent of Standard
validation and its PR-close companion.

## Release validation

`cargo-release-plan` compares released content with version anchors and owns the report schema
and version-readiness verdict. Its release baseline is the tip of the branch releases are made
from, which is not the branch a pull request targets, so the workflow passes the release branch
on a pull request, the merge-group base commit on a queue run, and the tested main commit
on main pushes and scheduled/manual runs.

`scripts/release/ReleasePlan.psm1` is the PowerShell boundary between that report and hosted
validation. Rust artifact commands validate the report and distinguish publishable release
assessments from all tracked version targets. The report's `packages`
array supplies released-content evidence and consumer-contract selection for publishable members;
`non_publishable_packages` supplies only names, declared versions, and derived group membership.
SemVer analysis and change-level decisions use the former, while grouping and alignment use their
union. The pre-apply publication gate resolves every planned target against current workspace
metadata and queries crates.io only for targets Cargo says are publishable. Package-name patterns
do not determine whether a crate has a consumer contract or is publishable.

`cargo-release-plan analysis-order`, `semver-targets`, and `propose` own dependency-order
presentation, consumer-contract selection, change-level validation, version-group realignment,
and release propagation. These operations consume captured artifacts without discovering the
workspace or contacting a registry. The module retains process orchestration and the crates.io
publication probe; the just recipes remain thin command-line entry points. Pester tests protect
argument forwarding, subprocess failures, publication checks, and the evidence lifecycle.

The guided release workflow collects its decision evidence after explicit offline preparation.
It then sends semantic choices to Rust's prospective resolution preview, which completes the
version-target set and captures the resolved files before application. The PowerShell boundary does
not duplicate Cargo resolution or infer binary closure membership. Compatibility evidence is
built with the prospective manifest path and working directory, so Cargo reads its captured
configuration and resolution rather than the live tree's. A read-only comparison rejects any
input mutation by that build. The module presents the stable
expanded artifact and applies it unchanged; the Rust boundary rejects stale original inputs
and installs only the captured state. CI's report/check path and post-apply reporting remain
read-only, with no hidden preparation or dependency refresh.

There is no separate version-approval prompt. The complete pull request and its
Version/release plan section carry the human review of release impact.

The unconditional `validate-versions` job shares one full-history checkout and environment
across live binstall metadata validation, version readiness and semantic-version analysis.
Release-target and archive-shape obligations follow Cargo's discovered binary targets,
including source additions that do not edit a manifest. Steps run in order: binstall validation,
version readiness, the SemVer canary and the scoped comparison. The comparison consumes the
version step's consumer-contract targets directly, which are emitted before its readiness
verdict. Binstall validation, readiness and the canary run independently after successful
setup. The comparison requires a successful canary and nonempty report-selected targets;
a missing version increment does not suppress it. Every failed check fails the combined job.

Rust plan-generation tests assert properties of the generated plan over a matrix of report
states, not only by testing individual guards. The properties are that every entry is well formed
and names a known target, that no target receives two decision kinds, that no version moves
backwards, that every version group ends on one version, and that no package keeps an
already-published version while a requirement inside it is rewritten. These are checked as
outcomes rather than isolated implementation guards. A scenario
passes either by refusing to generate a plan or by generating one that holds every property.

On Windows, the module scopes `CARGO_TARGET_DIR` for direct `cargo-semver-checks`
invocations to a stable, workspace-specific directory beneath the user
temporary directory. This keeps the SemVer tool's nested placeholder builds independent of
checkout depth without changing target-directory behavior for unrelated Cargo commands or
non-Windows validation. The override can be reassessed when
[cargo-semver-checks issue #1725](https://github.com/obi1kenobi/cargo-semver-checks/issues/1725)
shortens the generated paths upstream.

## Release publication

`release-plz` owns registry publication only. Its committed configuration disables Git
tag and release creation, so a main-advance race in GitHub publication cannot turn a
successful crates.io publish into a publisher retry. The publish job's ambient GitHub
token is read-only; Trusted Publishing retains its independent OIDC permission.

`scripts/release/ReleasePublication.psm1` owns Git/GitHub process orchestration after
publication. It derives package/version requests from the checked-out publication source,
reads existing remote tags, and builds `release-target-check` from that same controller
checkout only when a new tag needs a source candidate. The temporary candidate worktree
is data for the verifier, not the source of the verifier executable or automation scripts.

The nonpublished `release-target-check` utility owns candidate identity and version
constraints, and delegates released-content validation to `cargo-release-plan`. It requires
a clean checkout at the supplied immutable commit on the supplied main history, exact
requested package versions, and the release invariant against that snapshot's own anchors.
Using the candidate as the validator's baseline does not relax the invariant: the clean
worktree must still match each package's version anchor within that main history.

The PowerShell boundary uses the verified SHA in GitHub reference creation, confirms
the resulting remote reference, and retries only after observing that main moved.
It does not duplicate the Rust package-content or binary-dependency comparison.
Its temporary worktree is removed on both success and failure; cleanup failures retain
the original diagnostic rather than replacing it.

Missing binary releases use `gh release create --verify-tag` against the established
reference. Asset planning resolves each tag to a commit and carries `source_sha` in
the build matrix. Checkout consumes that immutable SHA; upload consumes the versioned
tag name. Existing references are not rewritten to match a newer preferred snapshot.
See [Release-equivalent snapshots](design.md#release-equivalent-snapshots) for the
identity contract and credential rationale.

## Merge-blocking result

The `required-checks` job is the intended single ruleset target. Each validation workflow's
fan-in contains every one of its merge-blocking jobs. `scripts/build/RequiredChecks.psm1` rejects failed,
cancelled, missing, and unknown dependency results. It permits `skipped` only for jobs whose
event, platform, or package scope legitimately excludes them.

For tooling checks it reads `prepare`'s explicit path plan and reconstructs script selection
using its affected-package output. The execution-domain output must agree with that
selection. Every selected tooling job must succeed; every tooling dependency must be present,
even when not selected. Preparation remains a must-succeed dependency, so a failed planner
cannot turn downstream skips into merge approval.

The classifier only observes what `needs` supplies, so it also rejects an unconditional gate
that its must-succeed list names but the payload omits. A name that drifts out of the `needs:`
list therefore fails the fan-in instead of silently disappearing from it.

The queue fan-in uses the same classifier with every dependency in its must-succeed list
and no `prepare` dependency. No queue job may skip. Repair branches use the same
repository/event conditions as other branches.

## Scheduled validation implementation

The scheduled scripts own planning, check invocation and readable reporting.
The App skills own diagnosis and ordinary issue/PR work. There is no shared state
machine connecting these components; their handoffs are GitHub reports and issues.

### Scheduled standard validation

The `standard` job calls `standard-validation.yml` after the main-only plan gate. Reusing
the workflow includes its complete check graph and platform matrices instead of maintaining
a nightly copy. GitHub preserves the caller's `schedule` or `workflow_dispatch` event in
the reusable workflow. Both scope planners select full-workspace/full-tooling outputs for
these events. The standard platform matrices are shared with PR and main-push validation.
All jobs check out the same event commit; main release validation uses that immutable commit
as its baseline.

The caller forwards the Codecov secret and grants the permissions declared by the called
jobs, including the test identity's OIDC permission. Main-branch federation works for these
events without an additional Azure credential. The standard push-only `alert` does not
publish a second issue during a scheduled run. The parent `report` depends on `standard`
alongside every deep job and collects failed nested jobs from the same Actions run.

Standard validation's scheduled/manual concurrency group includes the run ID, so frequent
main pushes cannot cancel nightly standard checks. The scheduled suite does not reuse
previous successful results or skip unchanged packages.

### Deep execution

The workflow's `plan` job reads the full check catalog and supplies the matrix as
a job output. Each `checks` job uses an ordinary checkout of the workflow's main
commit, installs the environment and invokes the same Just recipe used locally.
The catalog defines recipes, platforms, packages and shards; there
is no hosted selection of a different source commit or reduced scope.

Each execution leg runs independently with fail-fast disabled. Jobs combining independent
checks use the same setup-gated continuation as standard validation: release compilation
continues after release Clippy fails, and ARM benchmark smoke tests continue after test or
upload failures. Always-upload steps
preserve its readable summary and raw diagnostics even after failure. The thin
capture wrapper records the exact Just command and preserves its exit status.
Generic process capture owns stream handling and child cleanup, not checker behavior.
The result directory stays outside the source checkout so source-isolating tools cannot
copy live diagnostic streams or generated artifacts. The wrapper rejects source-local
output before creating files or starting the recipe. Git ignore rules are not an
isolation boundary: cargo-mutants can copy ignored files.

The workflow places results under the runner's temporary directory and shares that
explicit path between execution and always-upload. Local entrypoint invocations default
to a unique directory under the system temporary directory and print its location;
callers may supply an empty external output directory. Logs are written directly there,
so partial diagnostics survive interruption without a final staging/copy step.
Successful artifact preservation does not turn failed validation green.

Diagnostic appends share a bounded UTF-8 summary. Finalization reserves the actual
final-result footer within GitHub's per-step byte limit, shortening diagnostics at
a UTF-8 character boundary when necessary and retaining a visible omission notice.
This final boundary also covers execution-error text written outside the diagnostic
helpers. The wrapper owns the entire step-summary payload and copies the finalized
artifact bytes unchanged, without newline conversion, a new byte-order mark or
appending to earlier content. Raw logs and native mutation artifacts stay complete.

The Just recipes own toolchain selection, test runners, helper preparation and
configuration. Ordinary Miri therefore uses nextest and its `default-miri` profile
in both development and CI. The many-seed recipe computes its own shard range.
The catalog retains per-package shard counts for `miri-harder`, with every current
package using `1/1` to cover the full seed range on one runner. Job identifiers
follow `miri-harder-<package>-<shard-index>` so names match the invoked recipe.
`just mutants` runs cargo-mutants with its native unmutated baseline and accepts an
output directory for collecting artifacts. There is no scheduled-only Cargo
argument builder, target enumeration or mutation verdict derived from result files.
A successful empty shard is reported as no mutation work; the wrapper does not
reconstruct a test invocation or claim that a baseline ran.

The full workflow executes every night, without persistent coverage receipts or
successful-run reuse. Ordinary dependency/build caches remain available.

The release-build, example, dependency-policy, feature-powerset,
unused-dependency and ARM test jobs depend on the same main-only plan
gate and run their Just recipes over the full workspace. They retain their own
platform matrices; the release-build job runs release-profile Clippy before building in the
same environment. The ARM test job also provisions Valgrind for benchmark smoke
tests and uploads its JUnit results to Codecov. Their failures are reported from
Actions job logs rather than the deep-check wrapper's summary artifacts.

### Manual checks

Manually starting **Deep validation** on `main` runs the same full suite as the
schedule. The workflow has no selection inputs and does not run on PR heads.

Repair authors run relevant local checks and link results with their tested SHA in the PR discussion.
These are reviewed alongside normal required checks; Standard validation does not
run a special repair gate or recreate a worker's version edits. Unavailable local
platform coverage is disclosed for human review, not claimed as a passing result.

### Failure reporting

The `report` job depends on planning and every check matrix and runs on failure.
It uses the same main checkout as the other jobs. Its normal GitHub permissions
allow reading Actions results and writing an issue. Preinstalled PowerShell and
the GitHub CLI are sufficient, even when checker/toolchain setup failed.

The reporter reads the run's effective job results, including executions reused by
a job rerun, and collects available check summaries and failed-job log excerpts.
It does not require the overall workflow to finish before reporting. Missing artifacts or
inaccessible logs are explicit gaps in the report, not reasons to omit a failure.
Issue content contains observed failures and direct links, not serialized API
inventories. The renderer bounds diagnostic text per unsuccessful job, retaining
the start and end of the job-log and check-summary excerpts, visible omission
notices, and structured artifact destinations outside the clipped text. Short
metadata receives its allocation first, then verbose sources share the remaining
budget and are each clipped once; assembly does not discard another source's
opening or final context. Artifact destinations come from the Actions inventory,
not from matching wording inside a check summary. Full diagnostics
remain in the linked logs and artifacts. Fixed-size failed-job inventory pages
and complete diagnostic sections are packed into size-limited Markdown messages;
source verbosity cannot create an unlimited continuation sequence.

The visible attempt prefix and inventory/job headings identify completed sections
on resumption. A section is never split between messages, so an observed heading
means its entire snapshot was published. Existing snapshots remain unchanged when
the run completes, diagnostics become unavailable or packing of missing sections
changes. Human reports and partial reports without these headings receive the
bounded sections in their existing discussion rather than a replacement issue.

All reporter mutations are serial and paced, including label creation and duplicate
reconciliation. Explicit HTTP rate-limit refusals permit finite exponential retries
after the longer of the secondary-limit cooldown and server-provided retry/reset
deadlines. No reconciliation reads happen during that cooldown. Other write
failures, including lost responses and malformed successful JSON, are ambiguous:
the reporter reads back the expected effect, continues only if it is observed, and
otherwise fails without replaying that write. An unavailable reconciliation also
fails. A later invocation searches the existing issue and all comment pages before
publishing missing sections. Duplicate explanations are likewise reused if their
subsequent closure fails.

The publication limits and cooldowns are implemented in `ScheduledReport.psm1` and
`ScheduledGitHub.psm1`. They follow GitHub's
[REST API best practices](https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api).
Pester exercises large reports and partial, throttled and ambiguous publication
against fake GitHub responses with mocked waits, never live content-creation bursts.

An exact visible attempt link identifies an existing report in open or closed
issues. The current workflow attempt identifies the report, while each job's
execution identifies its diagnostic artifact. Successful validation does not close earlier reports.
Reporting errors fail the reporter job and remain visible in Actions. Recovery
can rerun that job through normal Actions controls. Rerunning all failed jobs may
also rerun checks; neither path needs a separate reporting workflow or journal.

Triage and repair use the workflows described in
[scheduled validation](../../docs/scheduled-validation.md). Their GitHub comments,
labels, assignees and PR links remain understandable without App installation or
access to an executor's local files.
