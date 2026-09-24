# GitHub workflows implementation

This guide maps the workflow design to the repository tools that implement it. User-visible
CI behavior and design tenets are in [design.md](design.md); command flags and step details stay
with the commands and workflow jobs.

## Workflow events and orchestration

GitHub cron times are UTC. Local App schedules are configured separately using
the installed App's scheduling controls.

| Workflow | What starts it | Responsibility |
|---|---|---|
| `deep-validation.yml` / **Deep validation** | Daily at 18:00 UTC, or no-input manual dispatch on `main`. | Execute full standard and deep main-branch validation, preserve diagnostics and report failures within one run. |
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

## Development tool bootstrap and caching

`setup-environment` restores caches before invoking `scripts/setup/install-just.ps1`
and then the ordinary `just install-tools` recipe. Both use the same verified binstall
bootstrap and publisher-first policy as local development. Bootstrap constants are loaded
through `scripts/utility/Constants.psm1`, also used by the pre-setup benchmark canary.
See [development tool installation](../../docs/build-and-tooling.md#development-tool-installation).

The Cargo tool cache owns the installed executables and their `.crates.toml`,
`.crates2.json` and `binstall` metadata. It excludes rustup proxies. Its key includes
the platform, runner image and tool pins/installer inputs; a same-image fallback may
restore older versions, which the installer reconciles before a new cache is saved.
This cache is separate from `rust-cache`, whose binary caching is disabled, so changing
a tool pin does not discard workspace compilation artifacts. Standalone lint tools and
Bicep retain their independent caches.

The install steps supply the job's ephemeral GitHub token for public release discovery.
Local installation does not require authentication or modify credential configuration.
No step bootstraps binstall through compilation or disables signature verification.

## Benchmark workflow artifacts

Folo delegates its ordinary benchmark job graphs to
[`folo-rs/cargo-bench-history-action`](https://github.com/folo-rs/cargo-bench-history-action).
A **reusable workflow** is a GitHub Actions workflow invoked by another workflow through
a job-level `uses:` reference. The action repository provides the following shared workflows:

| Folo caller | Reusable workflow in the action repository |
| --- | --- |
| `bench-history.yml` | `.github/workflows/history.yml` |
| `pr-bench-history.yml` | `.github/workflows/pr.yml` |
| `bench-history-backfill.yml` | `.github/workflows/backfill.yml` |

These shared workflows own preparation and collection; history and PR also own analysis,
report artifacts and GitHub publication. Backfill only writes measurements to configured storage.
Their jobs invoke the repository's root **composite action**, a bundle of steps
used through a step-level `uses:` reference. It installs and invokes `cargo-bench-history`
for measurements and analysis, and `cargo-bench-history-github` (the **companion**) for
workflow evidence and GitHub issue/comment management.

Folo's caller files retain only triggers, gates, permissions and parameter values. Azure
client and tenant IDs come directly from repository variables; measurement settings are
literal workflow inputs rather than calculated job outputs.
Folo selects source installation, building the tools from the checkout that started the run
rather than selecting individual tool versions.

The fixed `bench-history-setup` hook selects Folo's ordinary cached setup environment with
Valgrind enabled. The callers supply matching `rustflags` values; the companion appends them
to effective ambient Cargo arguments using child-only environment overrides. It honors
`CARGO_ENCODED_RUSTFLAGS` precedence and preserves argument boundaries, and uses the same
environment for collection and its machine-key query. No configuration job or flag-merging
script is required in a consumer repository.

The shared workflows also use an internal composite action at
`.github/actions/workflow-tools` in the action repository. It prepares the caller's checkouts,
runs the fixed setup hook when needed, and installs the companion through the same installer
as the root action. Consumers do not call or configure this internal action directly.
The workflows' `$/` action references resolve both composites at the called workflow's own
commit, independently of the measured checkout. Notification requires the companion;
installation/bootstrap failure remains visible and has no second publisher.

The companion turns the configured platform CSV into the matrix. For history and PR it also
supplies the collection job prefix.
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

Automation and measured source are separate for PR runs. The invocation checkout supplies
helpers, tool builds and configuration. A full real-head checkout supplies Cargo scope,
benchmark execution and git topology. Collection
passes its repository and the automation configuration explicitly; analysis passes that
repository and the frozen event head/base. This preserves real commit attribution without
requiring every open PR head to contain new automation files.
The source-built collector inherits the automation-selected toolchain while benchmarking the
frozen head; measurement provenance records that compiler.

Both analysis flows exclude stored dirty snapshots, matching the root action's clean-only
selection. Workflow results describe frozen clean commits rather than developer snapshots
for a matching branch.

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
or inconclusive from validated evidence; scope preflight supplies explicit empty scope when no
analysis is needed. Preflight and failed-state commands share run/attempt/head ownership,
and failed publication never replaces a completed report. Issue inconclusive/failed annotations
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

Manual Azure provisioning remains a separate maintainer operation through `setup-azure`.

### Benchmark backfill caller

`bench-history-backfill.yml` retains Folo's nightly trigger, main-only same-repository dispatch
gate and repository-wide non-cancelling concurrency group. It passes `lookback: 14 days`,
`minimum-age: 24 hours` and the optional `to_commit` override directly to the reusable workflow.
Identity variables and literal measurement inputs need no caller-side execution.

The shared workflow's `prepare-workflow --flow backfill` uses one injected clock snapshot and
the invocation's full first-parent history to calculate the range. Explicit `from`/`to`
ranges remain available to other callers. Preparation freezes selected refs to full commit
SHAs and emits a separate successful no-work result when no automatic endpoint is old enough;
the workflow then skips its benchmark matrix. This is distinct from a fork-policy skip.
It does not ask current-head Cargo metadata whether
there are benchmarks: historical workspaces can contain benchmarks absent from the invocation.
The matrix checks out the resolved `to` commit with full history, while configuration, the fixed
setup hook and source installation remain invocation-owned. The core tool validates and traverses
the inclusive first-parent range, preserving the selected project directory in its worktrees.

Folo pins the released v2 reusable workflow to an immutable commit and passes
`install-method: path`, `source-path: .`, shared exclusions, all features and the shared
repetition count and compiler stability flags. Fixed skip-existing behavior makes each platform
resumable. `max-commits: '1'` reaches the core as an attempt budget after existing-result
prefiltering; it does not change range preparation or first-parent traversal. The chosen budget
matches [observed nightly capacity](design.md#nightly-history-backfill), not a hard time bound.
Each attempted commit retains its full repetitions, storage and flush/cleanup; the final log
summary distinguishes stored, existing, empty, failed and deferred work.

The caller omits `ignore-errors`, retaining its strict false default. The v2 workflow has no
whole-job failure-suppression input or job-level `continue-on-error`. Its matrix keeps
`fail-fast: false` and the six-hour exceptional watchdog; genuine failures and timeout remain
unsuccessful job conclusions rather than successful bounded completion.

Backfill creates no receipts, analysis job, report artifacts, publication sink or public outputs.
Shared run/work concurrency prefixes differ from Folo's caller group; non-cancelling
`queue: max` project/platform queues serialize aliases of the same configured storage project
without deduplicating invocations by event or SHA.

## Reusable workflow canary

The reusable-workflow canary is a small end-to-end integration check. Its purpose is to catch
failures at the boundaries that local tests cannot execute: calling a workflow in another
repository, obtaining Azure credentials on hosted runners, storing historical measurements,
and passing receipts and reports through the history flow. It checks that the workflow and monorepo
tools work together without requiring published tool versions or a full performance run.

`benchmark-action-canary.yml` calls the action repository's `history.yml` and `backfill.yml`
at the revisions specified in their `uses:` references, with tools built from the Folo checkout
under test. Its backfill calls use the same released revision as the production backfill caller.
On a pull request, it runs only when the source branch belongs to `folo-rs/folo`, not a fork.
History publication is disabled; backfill has no publication.
Its standalone fixture writes deterministic Criterion artifacts through the existing faker
library instead of measuring elapsed time.

The canary is `workflow_call`-only. Standard validation invokes it as `benchmark-canary` and
includes that result in both `required-checks` and main-push failure reporting. Deep validation
reuses the same graph through Standard and forwards its required token permissions. Main
pushes and scheduled/manual main calls select full scope; PRs combine explicit fixture/tooling
paths with Cargo delta's transitive impact on the CLI, companion and faker consumers. Private
CBH partitions and lower-level libraries therefore need no duplicated path/package inventory.
The path plan records credential eligibility separately from relevance. Forks never call Azure;
they still execute the selected credential-free preflight. The merge queue retains its separate
shallow graph and the same required-check context, without this runtime canary.

Preparation runs `just verify-caller-fixture` before the hosted call. The shared PowerShell
boundary uses Git to require a clean checkout and full `cargo metadata --locked` in the standalone
fixture workspace. `--no-deps` does not resolve dependencies and is not a lockfile freshness
check. The fixture lockfile is independent of the root workspace lockfile; path-package version
changes require `cargo update --manifest-path .github/fixtures/bench-history-caller/Cargo.toml
--offline --workspace` before committing. Validation must reject drift, never refresh it during
measurement. Native integration tests demonstrate committed path-version drift, missing locks
and dirty inputs against actual Cargo and Git.
`just verify-lockfile` also invokes the shared locked-resolution check during version planning,
without demanding cleanliness while release edits are still being prepared.

The caller uses the existing test identity and storage account. A read-only configuration job
exports their non-secret identifiers without signing in: Azure login masks the client ID,
which prevents GitHub from exporting it as a job output. The separate storage job consumes
those identifiers but exports none. Collection depends on both jobs and receives identifiers
directly from configuration.

The storage job creates the fixture's dedicated container through data-plane access; it does
not provision Azure management resources or use production history. Both calls use that container,
with separate configured project identities. The history flow's Linux, Windows and
Apple Silicon macOS collection jobs store measurements and transport receipts. Its analysis
job reconciles platform evidence, analyzes the real frozen head and uploads reports.
The history verification job downloads that artifact and checks the expected synthetic
series and honest outcome/coverage outputs. Lack of a baseline is not mistaken for failure or
asserted to be clean.

For backfill, configuration freezes the real event head and its first parent as the inclusive
`to` and `from` endpoints. The shared workflow runs the nested synthetic fixture on Linux,
Windows and Apple Silicon macOS, using `.cargo/backfill_history.toml` to select the isolated
`reusable-backfill-canary` project in the existing test container.
Both real endpoints must have consistent fixture locks. A lockfile repair checkpoint followed
by its guard/workflow integration provides valid adjacent inputs; changing the range, discarding
dirty flags or weakening the clean-endpoint assertions would hide the defect.

The separate `verify-backfill` job queries `list runs --json` across all stored machine keys and
targets at the frozen endpoint. It checks the expected project and requires both current range
endpoints as clean stored commits in one comparable partition for every target. Additional
machine partitions and older runs cannot substitute for the selected range. The listing is
retained as test evidence even when its verification assertions fail;
it is not an output of the reusable backfill workflow, which has no analysis or report phase.
The verifier does not manufacture a workflow verdict or require a judged-clean analysis outcome.
`Assert-BackfillCanary.ps1` owns the query and assertions under script analysis.
`BackfillCanary.Tests.ps1` invokes that script with mocked Cargo output to exercise
its acceptance and rejection behavior without Azure access.

The hosted caller proves historical storage across native targets. Same-runner installed-tool
smoke coverage in the action repository proves skip-existing resumption without relying on
separate hosted invocations receiving the same hardware fingerprint.

An additional Linux rolling call supplies zero minimum age and a sub-second lookback,
selecting only the event head through the quiet-window fallback. Its isolated project and
endpoint query establish actual stored results independently of the explicit-range call.
A separate no-eligible call uses an age older than all repository history; job evidence must
show successful preparation and no executed backfill matrix. Neither scenario copies the
planner into the caller. Frozen-clock tests own exact cutoff and calendar assertions.
The no-work verifier matches the complete nested job-name segment, so Standard and Deep caller
prefixes do not hide executed work or substitute an unrelated preparation job.
The no-work probe uses its own configuration path because the reusable action keys concurrency
by that path; it must not compete with rolling collection for the same pending-run slot.

The reusable canary's always-run `result` job requires success from every configuration, storage,
collection, backfill and verification job. Only the inner no-eligible backfill matrix may skip;
its preparation and no-work verification must succeed. Missing, failed, cancelled or unexpectedly
skipped contract jobs fail the reusable call and the Standard fan-in.

Invalid input, failed collection, stale attempts and lifecycle mutation cases remain covered
by the companion's mock/native suites and the action adapter tests. A successful synthetic
source caller does not satisfy the action repository's exact published-installation gate.

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

### Bicep validation

The `test-scripts` job also executes `just validate-bicep` when the non-Cargo plan selects
maintained Bicep templates/parameters, `bicepconfig.json`, the compiler wrapper or shared
tooling inputs. Its step has an independent post-setup condition, so another check's failure
does not suppress compilation and a Bicep failure still fails the required fan-in.

The native compiler, its pinned API type catalog and the configured linter own validation.
The PowerShell wrapper only selects maintained inputs, invokes the compiler and forwards
SARIF warning/error verdicts under the workspace's zero-warning policy. It performs no
Azure authentication, resource lookup or deployment. Diagnostics are retained on failure.

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
The same reconstruction checks the explicit hosted-canary output against the path/trust plan
and affected consumers. A selected call must succeed; skipping is accepted only for an
explicitly irrelevant or credential-ineligible call, never for absent or malformed scope.

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

Mutation checks use `1/2` and `2/2` on Linux and Windows. The shared `checks` job
timeout is 360 minutes, allowing for cold-cache setup and the workload per shard.

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

All reporter mutations are serial and paced, including report creation and duplicate
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

Report lookup uses repository-scoped, open-issue title search, followed by the
exact prefix filter from
[run-report recognition](../../docs/scheduled-validation.md#run-report-recognition).
It never scans the general issue inventory or closed reports. All search pages
must be complete and within GitHub's accessible result limit before publication;
incomplete discovery fails rather than becoming an empty queue. Matching candidates
are deduplicated and ordered by issue number, then refreshed by number to reject
stale closed or retitled search hits before reading their content or discussion.
The same fixed prefix constructs generated titles. No label API or assignment
is needed.

An exact visible attempt link identifies the execution within an eligible report.
Body identity takes precedence over comparison links in comments; paginated
discussion can supply identity when the body does not. The current workflow attempt
identifies the report, while each job's
execution identifies its diagnostic artifact. Successful validation does not close earlier reports.
Search indexing may lag an ambiguous issue creation. A missing indexed match does
not authorize replaying that write; unresolved publication fails visibly. A
successful create supplies the issue number for subsequent comments without
rediscovery. Separate invocations can reconcile visible open duplicates, but
publication does not promise exactly-once creation or reuse after report closure.
Reporting errors fail the reporter job and remain visible in Actions. Recovery
can rerun that job through normal Actions controls. Rerunning all failed jobs may
also rerun checks; neither path needs a separate reporting workflow or journal.

Triage and repair use the workflows described in
[scheduled validation](../../docs/scheduled-validation.md). Their GitHub comments,
labels, assignees and PR links remain understandable without App installation or
access to an executor's local files.
