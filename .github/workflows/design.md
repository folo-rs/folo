# GitHub workflows design

The high-level design of this repository's CI/CD workflows: the patterns they share,
the tenets behind them, and how the pieces relate. Per-job mechanics live in inline
YAML comments and in the `just` recipes the steps call; this document stays high-level.
Ownership of the release-validation pipeline is in [implementation.md](implementation.md).

## Scheduled validation

The [scheduled validation contract](../../docs/scheduled-validation.md) separates
check execution, failure triage and repair. Each handoff is an ordinary GitHub issue
that a human or Local Copilot App agent can understand and act on.

The **Deep validation** workflow runs the full standard and deep suites against merged `main` on its
schedule, or when started manually on `main`. It is not triggered by PRs or forks.
Its failure-reporting job files a readable **Scheduled validation failed on &lt;date&gt;**
issue when planning or checks fail. A triager investigates all reported
failures and creates or updates separate problem issues. The report closes when its
failures have been accounted for; the problem issues stay open until resolved.

Problem grouping follows the cause or independently actionable symptom, not job
boundaries or log fingerprints. Infrastructure failures are problems too; checks
blocked by a failed prerequisite are not themselves evidence of source defects.
Existing human-filed issues can serve as the problem issues.

GitHub assignees, labels, comments and linked PRs record ownership and progress.
There is no off-GitHub coordination store or encoded issue protocol. Claims are
ordinary collaboration, with explicit release or handoff rather than time-based
takeover. Personally operated Local App automations perform triage and repair;
GitHub-hosted workflows do not invoke AI. Final approval and merge remain human.

### Shallow and deep validation

**Standard validation** runs the ordinary shallow PR and push checks.
**Merge queue validation** runs a lightweight full-workspace gate for combined candidates.
**Deep validation** runs the full standard and deep suites at the main commit selected by its event,
without affected-package or PR-platform pruning. It reuses Standard validation rather than
maintaining a separate copy of its checks.
Deep validation covers ordinary Miri, many-seed Miri, mutation testing and careful checks.
It also runs release-profile Clippy and builds (`build-release`),
example execution (`run-examples`),
dependency default-feature policy checks (`default-features-check`), feature-powerset
compilation (`hack`), unused-dependency checks (`machete`) and ARM64 tests and benchmark
smoke checks (`test-arm`). These lower-yield checks run
nightly or on manual dispatch rather than on every push.
Release-profile Clippy rarely diverges from dev-profile Clippy, release builds add little
beyond it, examples rarely change or break,
and dependency default-feature policy mistakes have limited impact and can be repaired
asynchronously. These checks therefore do not block merging.
Planning, check jobs and failure reporting belong to that same workflow.
The local entry points have fixed meanings: `validate-local` is shallow and
`validate-deep-local` is deep. Repair authors run relevant local deep checks against the
reviewed commit and link their results for human review. Repair PRs use the same
required checks and version validation as other PRs, without a special merge gate.

Local and scheduled deep validation share the same Just recipes. Scheduling chooses
scope and captures diagnostics; it does not implement different checker commands or
pass/fail rules. Necessary check behavior belongs in the shared recipes.

Many-seed Miri jobs use the canonical recipe name, `miri-harder`, in their job names.
Each selected package runs its full seed budget in one shard by default. Additional
shards are reserved for packages approaching the job timeout; scheduled validation
does not need extra horizontal scaling solely to shorten already-small jobs.

Nightly runs execute the entire standard and deep suites even on unchanged source. Build caches
remain ordinary performance aids, not receipts used to skip validation. Hosted
execution and reporting do not depend on the availability of a Local App.

### Failure diagnostics

Checker findings and execution failures make the Actions job and workflow fail.
Independent matrix jobs continue so one failed shard does not cancel the others.
Readable reports include useful diagnostics, source and direct job links; full logs
and tool artifacts supplement rather than replace the explanation. Setup failures
are reported even when no checker artifact exists.
Successful runs and cancellation without a failed job do not create failure issues.
Generated diagnostics remain separate from source inputs, including while a checker
copies the source tree for isolated execution. Partial logs remain available after
interruption, and genuine checker failures retain their status and artifacts.

Step summaries fit GitHub's upload limit including the authoritative final result
and exit code. Diagnostic truncation is visible and retains references to complete
artifacts; it does not change the check verdict.

Failed-run issues retain a bounded excerpt and direct diagnostic links for every
unsuccessful job, not whole verbose summaries. Omitted diagnostic text is explicit.
Publication is paced and resumable: retries retain the same attempt's report and
already-published sections, even if the original logs later expire. Known throttling
receives bounded backoff; an ambiguous write is not blindly repeated.

An empty mutation shard is explicitly reported as no work, not a passing baseline.
The shared mutation recipe runs cargo-mutants' baseline for nonempty shards.
Missing output is not proof of an empty shard. Reproduction instructions preserve
known invocation scope; interleaved Miri output does not justify inventing a failing
test or seed. An unexplained intermittent failure is not resolved by a green retry.

Repair branches follow the same validation and benchmark conditions as other
same-repository branches. Ordinary repository/event conditions apply; branch names
do not select permissions or opt out of jobs.

## Job granularity and gating

Validation groups short, related checks that share a runner environment into named steps,
avoiding queue and setup costs that outweigh the checks themselves. Preparation shares a
checkout and runner; version readiness and API compatibility share their release-validation
environment. Independent expensive checks retain parallel jobs when that improves feedback
time. Separate steps preserve failure attribution, and each job stops checking at its first
failure. Artifact collection and resource cleanup still run after failure. The local recipes
define the local check suites, while workflow jobs own execution cadence, platform selection,
prerequisites and evidence capture. Clippy stands in for a bare `cargo check` here: Clippy compiles the code as a
prerequisite to linting it, so a standalone `check` job would only re-prove what a green
Clippy already guarantees.

## Selective validation

Package-scoped jobs consume Cargo dependency impact, so a one-package PR does not rebuild
the workspace. Non-Cargo checks have independent change domains: workflow lint consumes
workflow and lint-tooling inputs, script analysis consumes scripts and analyzer inputs, and
script tests consume their owning automation domains and shared dependencies. A change that
touches only tooling must still receive the relevant checks even when Cargo selects nothing.

Script tests run the union of selected domains, including integration tests for affected
native helpers. Domain selection includes fixtures, configuration and shared consumers, not
just the file containing a test. Shared validation machinery changes exercise every tooling
check. Ordinary Rust source changes do not by themselves select unrelated tooling checks.

Selection covers the complete pull request, including deleted and renamed inputs.
An unavailable change set is an error, not an empty selection. Pushes to `main` and
scheduled/manual main runs use the full set as a backstop. The workflow itself always starts,
so required-check reporting does not depend on GitHub's workflow-level path filters.

The `prepare` job publishes both Cargo and non-Cargo scope. Its complete outputs are required
before downstream checks can run or be accepted as intentionally skipped.

Release validation (`validate-versions`) remains unconditional: release-plan generation compares every
publishable package's released content to that package's version anchor, not just to the PR
base. Live binstall metadata validation accompanies it because Cargo target discovery can
change release obligations without a manifest edit. API compatibility follows successful
version readiness in the same job using the report's consumer-contract selection.

## Platform strategy

Test passes are organised as an x86_64/ARM64 pair. The x64 pass carries coverage
instrumentation (which needs a nightly-only toolchain component), while the ARM pass
doubles as the MSRV pass and exists to exercise architecture-gated code that x86_64 runners
never compile. The x64 pass runs in Standard validation; the ARM pass runs in Deep
validation. macOS is Apple Silicon, so it rides the ARM pass. Scheduled Miri
coverage includes architecture-gated paths as declared by its manifest. Platform-agnostic checks
(formatting, workflow validation, script tests) run on a single Linux runner because their result cannot
vary by platform.

Not every shallow check earns its place on every pull request. The
full shallow matrix runs on each push to `main`, but pull-request validation prunes the rarely-informative
legs to cut runner cost, leaning on push-to-`main` as the backstop for what it drops. PRs run the
test and docs suites only on the x86_64 Windows and Linux runners. The macOS doctest and
docs jobs wait for a push to `main`, because re-running these platform-independent
suites on macOS is rarely informative.
Release-profile Clippy runs only in Deep validation. The
compile-oriented passes in Standard validation (dev Clippy and frozen-minimum check)
deliberately keep their macOS leg on PRs, because a cheap macOS cross-compile still catches
macOS-specific build breaks that the pruned runtime passes would not. MSRV *compilation*
therefore stays covered on every PR by `check-frozen`, which compiles all targets on the
MSRV toolchain against the frozen minimum-version lockfile even though the ARM MSRV test
pass runs in Deep validation. Because a push to `main` is the first place Standard
validation's pruned checks can fail,
that event — unlike a PR — files a tracking issue (see Failure alerting).

Only pull-request events select the pruned docs/doctest matrix. Main pushes and
scheduled/manual main runs use the full matrix and skip delta. Pull-request delta analysis
uses `origin/main`.

## Merge queue validation

The merge queue checks the combined candidate with full-workspace dev-profile Clippy,
formatting and version readiness. Clippy compiles all targets and features on Linux,
Windows and macOS. No affected-package selection precedes these checks. Minimum-dependency,
SemVer, binstall, runtime and other standard checks remain in PR and full main validation,
not in the queue gate.

This gate trades repeated pre-merge validation for earlier merges. Passing PRs do not
prove that their combined changes pass runtime tests. Full Standard validation on main
pushes provides early detection; scheduled validation repeats both the full standard and
deep suites and reports failures even when no new commits arrive. Neither backstop gates
publication: publish-on-merge can release a combined-change regression before detection.

The dedicated queue workflow handles `merge_group` exclusively and reports the same
`required-checks` name as PR validation. Enabling the queue also requires changing the
repository ruleset from strict branch-up-to-date checks to required merge-queue checks.
Workflow changes alone do not change that live repository policy.

## External type surface

A dedicated job fails validation when a library exposes an external type — one that is
neither a standard-library type nor defined by the crate itself (a type from another crate,
first-party or not) — in its public API without that type being listed in the crate's
allow-list. The intent is to catch *accidental* additions to the external surface (a leaked
dependency type, a forgotten `pub`), not to prohibit external types outright; an intentional
exposure is admitted by adding it to the crate's
`[package.metadata.cargo_check_external_types]`. The user-facing principle and the allow-list
mechanics live in `docs/external-types.md`.

The check drives nightly rustdoc's unstable JSON output, which pins an exact schema version,
so it runs on its own pinned nightly (`RUST_NIGHTLY_EXTERNAL_TYPES`) held separate from the
general nightly and bumped only in lockstep with the tool. Like the other package jobs it is
delta-scoped, iterating the affected crates one manifest at a time and leaving the tool's
`--skip-unsupported` flag to pass over crates it cannot document (proc-macro and binary-only).
Because a public API can differ by platform through cfg-gated items, the surface is verified
on both a Unix and a Windows target, so a Windows-only or Unix-only leak cannot slip through.
Two targets suffice because platform-variant public surface here is gated only on
`cfg(windows)`/`cfg(unix)` (Linux stands in for macOS) or hidden behind a platform abstraction
layer with an identical public facade, and nothing public is `target_arch`-gated;
`docs/external-types.md` records the assumption and when the matrix must grow.


## Concurrency

Commit-driven and PR-driven workflows cancel superseded runs, keyed on the ref, so pushing
a new commit abandons the outdated run. That supersession only fires when a *new commit*
arrives on the branch, so closing or merging a PR — which pushes nothing to the PR branch —
would otherwise leave its in-flight Standard validation run to burn to completion. A dedicated
companion workflow closes that gap: it triggers on the PR-close event and joins the target
workflow's concurrency group so cancel-in-progress reclaims the stale run. Standard validation
uses that close companion. PR benchmark history handles `closed` in its own workflow: the
event enters its group and skips collection and publication. Unsupported fork events use
separate groups, so a same-named fork branch cannot cancel supported benchmark work.
Merge queue validation has its own queue-ref-specific group. Standard validation uses
run-specific groups when called by scheduled/manual validation, so neither main pushes nor
other scheduled runs cancel that full-scope backstop. The close companion stays
pull-request-only. The exception
is history collection on `main`, whose workflow-level group is keyed on the commit **SHA**:
each commit is a distinct measurement, so only a redundant re-trigger of the *same* commit
is deduplicated. Manual re-collection also keys on its repair target so different historical
points remain independent.

Push-to-main history collection runs at most one job per platform across workflow runs.
Linux and Windows have separate queues, so one platform does not wait for the other.
Distinct commits wait without occupying runners or cancelling running or pending collection,
up to GitHub's queue capacity. The limit covers only push-triggered collection: manual runs
use run-specific collection groups, and downstream analysis does not hold a collection slot.
Workflow-level deduplication still applies to redundant runs of the same history point.

A schedule-driven workflow carries a concurrency block only when a duplicate run would be
expensive: the nightly history backfill groups on itself with
cancellation **off**, so a manual dispatch queues behind the scheduled run rather than
duplicating hours of benchmarking. Keeping it out of collection's SHA-keyed group matters for
the same reason — a scheduled run's SHA is the current tip, so a shared group would let the
nightly and that tip's own collection cancel each other.

PR benchmark collection additionally runs at most one job per platform across the repository.
Linux and Windows use separate worker pools, so they do not block each other. Other PRs wait
in the platform's concurrency queue without cancelling running or pending collection, up to
GitHub's queue capacity. Ref-keyed workflow supersession and the close event still cancel
outdated work, including collection waiting for a platform slot. This limit applies only to PR
collection; delta analysis and comment maintenance do not wait for a collection slot, and main
history collection and backfill retain their independent concurrency policies.

## Thin steps

Workflow steps stay thin so their logic can be exercised locally. Nonpublished Rust utilities
own structured parsing and policy logic wherever the calling environment can execute Rust.
PowerShell handles boundaries where that is impractical, including toolchain bootstrap and
native App coordination without a prepared Rust environment. Thin `just` recipes expose the
commands; reusable PowerShell orchestration belongs in Pester-tested modules under `scripts/`.
The [automation language guidance](../../docs/build-and-tooling.md#automation-language-and-boundaries)
defines that boundary.

Steps implementing a design obligation explain the reason beside the step or cohesive step group
and link to its owning design or implementation heading. This keeps authority, ordering and
failure-handling decisions visible without duplicating their full rationale. Every `run:` step
uses `pwsh`; the `setup-environment` composite is the sole Bash exception because it bootstraps
PowerShell itself.

## Pull-request version readiness

Published content changes require an explicit **change level** — `breaking`, `nonbreaking`, or
`patch` — decided before release. The change level describes the substance of the change;
tooling maps it to a Cargo increment level or an exact target version. This applies to the
workspace as a whole rather than only packages selected by delta analysis: an earlier change can
remain pending even when the current pull request does not touch that package.

Release state is read from the branch that publishes, not from the branch a pull request
targets, so a stacked pull request is assessed against the same baseline as any other and a
parent branch's pending increment is never mistaken for a release. A merge-queue entry is the
exception: it is assessed against the commit the queue rebased it onto, so its scope matches
what will actually land.

Version increments follow Cargo's compatibility rule rather than plain semantic versioning:
the leftmost non-zero component acts as the major component, so a compatible change to a 0.y
package advances its patch component and a 0.0.z package has no compatible increment at all.

Automated API comparison supplies evidence for those decisions but does not replace semantic
review. It is fail-closed when its input format or execution is unsupported. Comparisons cover
only packages that present a consumer contract, which a package states in its own manifest by
declaring a private API or by saying nothing. Published implementation and test-support packages
declare themselves private; changes in a grouped
implementation package are assessed through the owning public package instead, which loses
nothing because a re-exported item appears in that package's own API.

The checks support a valid empty consumer-contract set without turning that case into a
workspace-wide comparison.

A change level rests on released evidence rather than on the version a manifest already declares.
A package's own released-content diff, the workspace values it inherits, the locked dependencies
an executable releases, and the decisions taken for its dependencies all participate, and any
package-metadata change establishes at least `patch`. A version group whose members disagree is
realigned mechanically and needs no change level of its own. Every tracked member contributes to
the highest version and receives the resolved version, including members with publication
disabled. Released-content protection applies only to publishable members: alignment advances
the whole group when retaining an already-published version would rewrite one of those members.
Only publishable packages can take the first-publication path. The [`increment-versions`
skill](../skills/increment-versions/SKILL.md) carries out this policy and owns the procedure,
and [`docs/release-versioning.md`](../../docs/release-versioning.md) is the chapter that
governs it.

Two consequences of a package's manifest are checked directly rather than left to that review.
Every requirement on another workspace package names the exact version its target declares, so a
released manifest describes the combination the workspace built rather than a range it never
resolved. Exact `=major.minor.patch` requirements between workspace members declare version-group
edges. Their undirected connected components determine the groups, so not every dependency within
a group must be exact. Incrementing a package therefore also increments its in-workspace
dependents whose manifests the rewrite changes. And a package whose public API exposes another
workspace package must release a breaking change whenever that package does, because an
incompatible release changes the identity
of the exposed types for consumers. Which dependencies are public is read from the
`allowed_external_types` allow-list that the external-types check already verifies, so this rests
on a declaration the repository maintains rather than on a second inference of the public API.

A requirement of the wrong form is a manifest defect rather than a missing increment, so it is
corrected by editing the requirement.

## Required checks fan-in

Standard and Merge queue validation post a fan-in job whose GitHub check name is the ruleset string.
Their event triggers are disjoint, so each PR or queue candidate receives one merge gate. GitHub's
required-checks field is a string match on that name: it cannot express "this matrix
job, but only the legs that actually ran", and it cannot see a check that was skipped
rather than posted. A job with both `strategy.matrix` and a job-level `if:` that evaluates
false never expands the matrix, so contexts such as `test-x64 (ubuntu-latest)` stay on
Expected — Waiting for status to be reported forever if they are listed as required.

A ruleset that requires merge-blocking Standard validation therefore lists only this fan-in. The
job is `if: always()`, `needs:` every merge-blocking job in Standard validation (including
`prepare` and `validate-versions`), succeeds when every dependency reports `success` or an
allowed `skipped`, and fails on `failure`, `cancelled`, or any other result.
Unconditional gates may not skip. Change-selected tooling jobs must succeed when selected;
only an explicit no-work plan permits them to skip. Missing plans or dependencies fail the
fan-in. Advisory jobs stay off that list. `alert` stays off it
— it files issues on a failed push to `main`, it is not a merge gate.

When a new merge-blocking job is added to Standard validation it is added to this `needs:` list; it
is never added to the GitHub ruleset. Unconditional gates are also named in the fan-in's
must-succeed list. Matrix jobs that can skip via a job-level `if:` can only be made
required through this fan-in.

The queue fan-in requires every queue check to succeed; none may skip. It uses the
same result classifier without a change plan, because queue scope is unconditional.

`alert` keeps a `needs:` list of its own, which also names the advisory jobs the fan-in excludes,
so a new job joins both. The two lists answer different questions — what blocks a merge, and what
is worth an issue after a push to `main` — and folding `alert` onto the fan-in would tie issue
filing to the fan-in's skip policy and begin filing issues for cancelled runs.

## Published user guides

Markdown user guides live under `packages/<package>/book/` and are published together as one
GitHub Pages site. The book workflow discovers that convention rather than keeping a second
catalog, builds each book in an independent matrix leg, and merges the resulting artifacts under
`/<package>/`. A generated landing page at the site root reads each book's title and description
from `book.toml`, so adding a book requires no workflow edit.

Pull requests build every discovered book but never deploy, providing a real rendering check
without publishing unmerged content. Pushes to `main` and explicitly dispatched runs assemble the
artifacts, generate the landing page, and deploy through the GitHub Pages artifact flow. mdBook and
its preprocessors are version-pinned in `constants.env` so local and hosted builds use the same
rendering toolchain.

## Coverage reporting

Coverage is a side effect of the ordinary test run, not a separate re-execution. A single
commit therefore produces several coverage uploads — one per platform, plus a conditional
upload from the Azure-backend job. A platform upload is itself conditional on a report
existing: a delta-scoped run can measure only packages that carry no instrumented code, which
yields nothing to report, nothing to upload and nothing to notify about. Codecov is configured
to hold all notifications until a final gate job signals that every expected upload for the
commit has landed, so the reported figure is computed from the complete set rather than
flapping as partial uploads arrive. The gate keys off "every expected upload succeeded or was
legitimately skipped", never off a hardcoded upload count, because the Azure upload is
conditional; when no coverage landed at all, it releases nothing.

## Azure backend testing

The `cargo-bench-history` Azure storage backend is validated in layers of increasing
fidelity, each an additive sibling of the last: against a local Azurite emulator (the layer
that also feeds coverage), against a real Azure Storage account with shared-key access
disabled (proving real Microsoft Entra ID signature validation the emulator fakes), and
against that same account through the tool's self-minting GitHub OIDC credential (the exact
path the production history collection depends on). The backend's network paths self-skip
when no emulator or account is reachable, so the ordinary multi-platform test jobs stay
green without one; the Azure jobs flip that skip into a hard failure so a misconfigured job
can never silently pass by testing nothing. All Azure authentication uses GitHub OIDC
workload-identity federation — no long-lived secret is stored — and is gated to same-repo
runs by explicit workflow policy.

Real-Azure authentication modes share a job and run sequentially against independently
created test containers. Each pass selects its credential through step-local configuration;
the application's self-minting mode does not use the developer session for storage access.
The shared login remains available for test cleanup.

## Federated identity

Every Azure sign-in in these workflows uses GitHub OIDC workload-identity federation, so no
long-lived storage secret is ever committed or held as a repository secret. A job requests a
short-lived GitHub OIDC token (`permissions: id-token: write`), and Azure exchanges it for
managed-identity credentials only when the token's *subject* matches a federated credential
registered on that identity. The subject encodes the triggering event: a push to a branch
presents `repo:folo-rs/folo:ref:refs/heads/<branch>` (e.g. `…:ref:refs/heads/main`), while a
pull-request run presents `repo:folo-rs/folo:pull_request`. The audience is always
`api://AzureADTokenExchange`. The two non-secret identifiers a job needs — the managed
identity's client id and the tenant id — live in `constants.env` and are remapped to the
standard `AZURE_*` names by the shared federation helper. Production writers select
`AZURE_PROD_CLIENT_ID`; readers select `AZURE_PROD_READER_CLIENT_ID`. Reader selection never
falls back to the writer, and using the same client ID for both is a configuration error.

Azure-touching work is restricted to **same-repo** PRs by an explicit head-repository check.
The `pull_request` subject does not encode whether the head comes from a fork, so subject
matching and the absence of stored secrets do not replace that workflow gate.

Managed identities separate durable production writes, production reads, and disposable tests:

| Event | OIDC subject | Identity | Consumer |
| --- | --- | --- | --- |
| push to `main` | `…:ref:refs/heads/main` | prod writer | History collection |
| schedule on `main` | `…:ref:refs/heads/main` | prod writer | History backfill |
| push/dispatch on `main` | `…:ref:refs/heads/main` | prod reader | History analysis |
| pull request | `…:pull_request` | prod reader | PR analysis |
| push to `main` | `…:ref:refs/heads/main` | test | `test-azure` backend tests |
| schedule/manual dispatch on `main` | `…:ref:refs/heads/main` | test | Scheduled `test-azure` backend tests |
| pull request | `…:pull_request` | test | `test-azure` backend tests |

`merge_group` is not a trusted subject. Queue validation does not include `test-azure`,
avoiding an exchange that cannot succeed.

The production writer retains `Storage Blob Data Contributor` for collection and backfill.
The production reader has `Storage Blob Data Reader` scoped to the history container.
PR collection holds no Azure federation permission and writes only run-local files. Analysis
reads those files together with the production baseline through `--local-input`; publication
runs in another job with GitHub write scopes and no Azure federation.

Deployment is reader-first: add the reader and record its non-secret client ID before
activating its consumers. Preserve an existing writer PR credential until legacy PR-writing
runs have drained, then explicitly retire it. Incremental ARM deployment does not delete an
omitted credential. Fresh writer identities are main-only, and later deployments preserve
retired trust rather than recreating it. The deployment wrapper owns that policy; see
[`infra/azure-bench-history-prod`](../../infra/azure-bench-history-prod/README.md).

## Benchmark history

History collection runs on every push to `main` rather than on a schedule, so it captures
exactly one data point per commit instead of re-measuring an unchanged tip and missing
intermediate commits. It writes to a dedicated production storage account under a dedicated
production managed identity, kept entirely separate from the throwaway account the test jobs
use, so the long-lived data store never depends on test infrastructure. Collection is
append-only and idempotent, which is what makes a re-run safe and lets a read-through cache
of the bulk history persist between runs.

Collection excludes the slow, special-purpose `benchmarks` package and the deprecated
`infinity_pool` package. `infinity_pool` is retained for legacy use, not ongoing performance
development, so measuring it would consume CI time and regression-triage effort without
supporting active maintenance goals. Main collection, re-collection and nightly backfill use the same
package exclusion list; PR collection removes those packages from its affected set before
deciding whether there is anything to measure. Deprecation does not require deleting a
package's benchmark suite.

Analysis considers only benchmark identities measured at the queried context commit. Once
an excluded package is absent there, its stored historical series are dropped before
detection, so old regressions do not remain in current reports. Those measurements remain
available when explicitly querying a historical context where they were collected; neither
blessing nor deleting stored history is part of a collection exclusion.

Collection stamps every engine's results with the runner's **own auto-detected hardware
fingerprint**, with no fixed key override. The GitHub-hosted pool is heterogeneous, so a
single shared key would blend genuinely different machines into one jittery series;
fingerprinting instead splits the pool into one clean series per hardware type. Because
collection is a matrix and analysis is a single job that cannot re-derive those keys from its
own hardware, each collect leg writes a run/attempt-bound receipt with its fingerprint and the analysis job
threads exactly the successfully collected keys into its selection — scoping the analysis to the
machines that actually measured this commit, without assuming anything about other data in
the shared store. The cost of that split is sparseness: consecutive commits land on whatever
hardware the pool handed out, so each per-key series sees only a fraction of `main`'s commits.
The nightly backfill below exists to densify them.

To blunt the residual within-machine jitter, collection runs the whole suite several
times per commit and keeps, per metric, the minimum sample: runner interference is one-sided
(a contended host only ever makes a benchmark slower) and the repeats are spaced apart in
time, so the minimum is the reading least perturbed by transient noise. This trades a
proportionally longer collection job for a more stable series.

Both this workflow and its PR variant benchmark only the x86_64 Linux and Windows runners.
ARM and macOS are only nominally supported — they must pass tests (see the Platform strategy
section) but their performance is not tracked — so benchmarking them would spend runner
minutes producing series no one reads.

Every selected package is benchmarked with all Cargo features enabled. This makes Cargo include
benchmark targets guarded by `required-features` and builds each selected package in its
all-features configuration. The push, PR, re-collection and nightly-backfill paths all obtain this
feature selection from the same command builder, so a stored point is never made incomparable by
one path using narrower feature coverage.

Even with those defences, a single day of a badly degraded runner can still leave one commit's
data point corrupted. Collection is therefore also manually re-runnable against a specific
historical commit: a `workflow_dispatch` with a `recollect_commit_id` re-measures just that
commit and *overwrites* its stored point instead of appending the pushed tip. The subtlety this
resolves is that the collection tool lives in the same repository as the benchmarks, so a naive
"check out that commit and re-run" would also run the tool as it shipped at that commit. Instead
the re-collection benchmarks the code *at* the target commit in a throwaway worktree while
running the current tool, so the measured code and the compiler that builds it come from that
commit while the collection logic does not. The overwrite bumps the cache-invalidation marker
so downstream analysis refreshes,
and it deliberately discards any blessings recorded at that commit, since a fresh measurement
invalidates a level that was previously accepted. Analysis is unaffected by the input and always
surveys the current `main` tip.

The stored history can also change *out of band* — a blessing or unblessing, a `prune`, or an
administrative overwrite performed from a developer machine. Those surface in the rolling issue on
the next push, which re-lists the store (so out-of-band additions are seen) while deletions and
overwrites bump the cache-invalidation marker (so those are seen too). There is deliberately no
"analysis only" dispatch mode: analysis threads the *exact machine keys collected this run* from the
collect matrix into the single analyze job (see below), so a mode that skipped collection would have
no keys to analyze. To force a refresh out of band, push a commit or dispatch a `recollect_commit_id`
run (which still collects, hence still produces keys).
A downstream analysis job reads the accumulated
history and publishes through a separate GitHub-only job. A rolling, advisory issue is
identified by its instance/kind marker, not its mutable title or a label. History preflight
marks old findings stale; findings replace the report, while fully judged, complete clean
evidence writes all-clear without automatically closing the issue. Incomplete or unjudged
analysis never clears findings. Regressions never fail the run.

Partial collection is disclosed in the published body without suppressing findings. Receipts
are reconciled with each platform's latest job attempt: failed retries cannot reuse old
successful receipts, and untouched successful legs remain usable across partial reruns.
Total collection failure is an error, not a fabricated non-notable report.

Because a GitHub issue body is size-capped and a large
analysis can exceed it, the issue carries a **condensed summary** (the top findings) and
links to the **full report bundle**, uploaded for every completed analysis — so the issue
fits while complete results, including quiet or partial reports, remain accessible.

### Nightly history backfill

A scheduled companion workflow densifies the per-machine-key series the push workflow leaves
sparse. At 02:00 UTC — clear of the cache warmup's midnight slot — it runs the collection tool's
`backfill` in its default skip-existing mode over a window of recent `main` commits, on the same
two platforms, so whichever machine key its runner draws that night receives the newest commits
that key is missing. It is purely a producer: it performs no analysis and raises no alert.
Analysis stays with the push workflow, which surveys a densified series the next time one of its
runners draws that same machine key.

The window is computed per run rather than fixed. Its newest end is the newest first-parent
commit at least 24 hours old, which keeps the nightly from racing a push-collect that may still
be measuring a recent commit (collection runs for hours). Its oldest end is 14 days
back: history older than that has no comparison value against the current tip, since detection
reads only a short window of recent points, and the same bound caps how far back the
measurement-configuration caveat below can plant an odd-looking point.

Being killed by the clock is the expected outcome, not a failure. One commit costs as much as a
push-collect and more — the backfill worktree's build directory sits outside the shared
dependency cache, so it always builds cold — so a night fills roughly one gap per platform
against the six-hour hosted-runner ceiling. The job therefore carries the maximum
`timeout-minutes` *together with* `continue-on-error`, which is what turns the kill into an
unremarkable end rather than a red scheduled workflow, and it ignores per-commit errors so one
unbuildable commit does not end the walk. Because backfill works newest-first, whatever the run
managed to finish is the most comparison-relevant part of the range. A kill landing in the
seconds a commit spends writing its per-engine results leaves that commit stored for only some
engines, and later runs count it as filled; repairing it takes a `backfill --overwrite` over
that commit. The accepted cost is that a
genuinely broken nightly — bad credentials, a tool bug, every commit failing — is equally green
and silent; this is an opportunistic job, and such breakage still surfaces within hours in the
push workflow, which does alert.

It carries its own concurrency group instead of joining history collection's. A scheduled run's
SHA *is* the current tip, so sharing that SHA-keyed, cancel-in-progress group would put the
nightly and the tip commit's own collection into one group where whichever started later kills
the other — hours of benchmarking discarded in either direction. The backfill's own group merely
stops a manual dispatch from duplicating a scheduled run, queueing it instead. The dispatch
exists as an escape hatch: it can override the computed newest endpoint to step over a commit
that fails slowly and would otherwise be re-selected every night.

Two fidelity caveats ride along, both worth recognising before an unexplained step in a series
is read as a real regression. A backfilled commit is built with the toolchain that commit pins,
but its `RUSTFLAGS` and benchmark scope come from the current checkout — they are caller intent
that a general-purpose tool cannot recover from a historical worktree — so a commit older than
the newest change to either is measured slightly differently from its pushed neighbours; the
14-day window is what bounds this. Separately, the hosted runner images roll weekly and the
hardware fingerprint does not capture the image version, so a gap filled tonight may be measured
on a newer image than the neighbour it sits between — an exposure the pushed series already
carries, and strictly better than leaving the gap empty.

### PR benchmark history

PR feedback answers whether the frozen PR head changes benchmark performance relative to its
base. Automation is built from the event's merge checkout so updated helpers are available
to older PR heads. Benchmark execution and topology use a separate full checkout of the real
head; no measurement is stamped with the synthetic merge commit. The event's frozen head and
base frame the comparison.

Collection is **delta-scoped**: cargo-delta compares the measured head with its merge base,
expands impacted packages to dependents, and the shared collection exclusions remove packages
that this workflow does not maintain. An empty scope routes directly to an explanatory
comment, without collecting or requiring Azure configuration.

The collection matrix has no Azure federation or GitHub posting permission. Each successful
leg uploads its local store and a run/attempt-bound receipt. Analysis combines only validated
successful inputs with the production baseline through `--local-input`, using the reader
identity. It restores the main history cache without saving PR entries; artifact staging is
outside the persisted cache path.

Analysis remains unscoped by package name. Benchmark identities are engine-dependent, so
name-prefix filtering could drop valid measurements. The tool's always-on ghost filter
limits detection to identities present at the measured context, and local run objects take
precedence over matching baseline objects.

Findings land in a single **rolling PR comment**, identified by a hidden marker. It reports
both improvements and regressions, and discloses package scope, missing collection platforms,
and unjudged series independently. Named outcomes distinguish clean evidence from insufficient
baseline or nothing in scope. A total collect failure produces no fabricated report. Findings
remain advisory and never change the process exit code.

A preflight job runs after scope and companion preparation, without waiting for a collection
slot. It seeds or refreshes an owned in-progress placeholder when results are not yet available,
or adds a replaceable staleness banner to older results. The analyzed commit is retained as a
full machine marker and a human-readable commit link. Unknown commit distance is disclosed
rather than presented as fresh.

Analysis and publication use `!cancelled()` so superseded work does not publish. The separate
GitHub-only publication job also checks the live head immediately before writing: stale or
unverified freshness is qualified, and already-current newer results are preserved. Comment
writers share one per-instance/per-PR concurrency group.

Empty-scope cleanup creates or updates a brief explanatory note instead of silently deleting
the comment. A finalizer may run after failure or cancellation, but changes only the placeholder
owned by that exact run and head; it never replaces real results or a newer run's placeholder.
The close event enters workflow concurrency without starting benchmark or posting jobs.

## Failure alerting

The push-triggered history collection, release, and validation workflows all open a GitHub
issue on failure, but with
deliberately different lifecycles matched to what failed. A benchmark-history failure is
a recurring condition on a rolling target, so it opens a *deduplicated* tracking issue
(keyed on its instance/failure marker, with no label requirement) that the companion closes automatically once the workflow is
green again — exactly one open issue per persistent failure, cleared without manual
intervention. The nightly backfill is deliberately outside this scheme and files nothing (see
Nightly history backfill). A release failure is a discrete event tied to one publish attempt, so it
opens a *per-run* issue (identified by the failing run) that stays open until a human
investigates; each failed release is tracked individually rather than folded into a
rolling issue. A push-to-`main` Standard validation failure follows the same per-run shape as the
release alert — a fresh `ci-failure` issue per failing run, no dedup and no auto-close —
because it now backstops the checks pruned from PR validation, so each such failure warrants
individual triage. It fires *only* on push to `main`: a PR failure is already self-evident as
the red check and needs no issue, so the alert is gated on the `main` ref (which a
`pull_request` run never presents) and on `failure()`, leaving a green or skipped-only run to
file nothing.

## Release automation

Merging reviewed version increments to main publishes their packages to crates.io and
reconciles GitHub tags, binary releases and cargo-binstall archives. The operational flow
lives in [`docs/release-automation.md`](../../docs/release-automation.md).

### Release-equivalent snapshots

A package's version anchor identifies the main commit that introduced its version. It
remains the comparison baseline for version validation, not a mandatory release-tag target.
A release tag identifies an immutable main snapshot containing the package's released
content at that version. A later main commit is equally valid when the package version
and its release-relevant content remain unchanged.

This follows from the merge gate: released-content changes require a version increment.
Equivalence uses the same package-content model as that gate, including inherited manifest
values and an installable binary's locked dependency closure. It does not require identical
unrelated workspace files, workflow files or build environments, and does not promise
byte-identical rebuilt binaries. A crate already on crates.io is never republished;
its recorded source commit can differ from the equivalent snapshot chosen for its
GitHub release and prebuilt binaries.

GitHub can require workflow-write authority when creating a tag at a historical commit
whose workflow files differ from main. Actions' ambient token cannot receive that
permission. Requiring every tag to point at its version anchor would therefore make
unattended recovery depend on a permission the workflow does not possess. Selecting a
verified current-main snapshot preserves package identity without adding credentials or
blocking unrelated merges.

### Publication and recovery

Registry publication and GitHub publication have separate owners. Release-plz publishes
crates through Trusted Publishing but creates neither tags nor GitHub releases. A shared
reconciler handles both ordinary GitHub publication and recovery after a partial or manual
registry publish. Libraries receive tags; publishable binary packages also receive GitHub
releases and prebuilt assets. Discovery remains package-driven rather than a hardcoded list.

The reconciler freezes the package/version requests from the successful registry
publication's source snapshot. Before creating missing tags, it fetches main, pins its
commit, and verifies a clean disposable checkout with the release validator. Every requested
package must still be publishable at exactly the requested version. A version string alone
does not authorize content that fails the release invariant.

Writes use the verified commit ID, never an unchecked moving `main` reference. If tag
creation fails and main has advanced, a bounded retry selects and verifies a fresh snapshot.
An unchanged main, failed verification or exhausted retry budget surfaces an error.
Advancement to a different package version is not permission to relabel that version:
automatic recovery of a superseded version is not guaranteed.

Existing tags are authoritative and are never moved or overwritten. A missing binary release
is attached to its existing tag, without asking GitHub to choose another target.
Binary build jobs receive the tag's resolved commit ID separately from the release name,
so source checkout remains pinned while assets are uploaded to the correct versioned release.
Partial successes survive a retry; reconciliation creates only what remains missing.

## Cache warmup

A scheduled workflow recompiles the shared dependency cache on every runner image daily so
it is never evicted for inactivity. Without it, a cold cache would force every parallel
validation job to compile all dependencies from scratch.

## Shared infrastructure

All non-trivial jobs use the `setup-environment` composite action to install a single,
consistent toolchain (`just`, PowerShell, the Rust toolchain, and release tooling);
deviating from it to hand-pick a minimal per-job toolchain costs more in maintenance than
the mostly-cached setup time it would save. Toolchain versions are defined once in
`constants.env` and `rust-toolchain.toml` and reach the workflows through the `just`
commands they call, so no version is ever duplicated into a workflow file.

The one deliberate deviation from "one identical environment everywhere" is Valgrind. It is
installed only where a job actually executes Callgrind measurements — the benchmark
collection jobs, the test jobs that smoke-run every bench target, and the cache warmup that
primes their caches — because Valgrind pulls in glibc debug symbols that are pinned to the
exact glibc build on the runner image. Installing it in every job would put every Ubuntu job
in the repository at the mercy of routine Ubuntu security updates, so the opt-in confines
that exposure to the jobs that cannot work without it. For the same reason the APT package
cache is scoped to the runner image version, so it rolls forward with the image instead of
serving debug symbols that no longer match — and because that scoping makes every image roll
resolve packages afresh, the APT index is refreshed on every Linux job rather than trusted as
the image left it.

## Transient-fault handling

CI touches unreliable infrastructure — package mirrors, the GitHub API, runner disks — where a
single blip (an HTTP 5xx, a rate-limit refusal, a dropped connection, a runner disk I/O error)
is not a real failure and must not fail a whole job. Such faults are retried automatically, and
at the lowest feasible level: one flaky download or one API read is re-attempted in place rather
than restarting the job around it. Shell bootstrap and download callers share
`scripts/utility/Retry.psm1`: `Invoke-WithRetry` re-runs an action a few times with exponential backoff
capped at a ceiling, and `Test-TransientFailure` classifies an error message so callers can retry
only genuinely transient faults.

Retry wraps the operations most exposed to that risk: the in-repo Rust toolchain install (the
`rustup toolchain install` a runner disk blip once failed with no retry, dropping a whole job on
one bad sector), the actionlint/shellcheck/azcopy tool downloads (which re-fetch *and* re-verify
the checksum, so a truncated payload re-downloads rather than being trusted). The Rust
benchmark companion owns the equivalent typed HTTP policy for its GitHub operations.

Non-idempotent creates never retry blindly: the companion reconciles a failed response
against the intended marker and body. Reads, complete-body updates, closes, and deletion of a
known comment are idempotent and retry transient failures within a bounded policy. A repeated
delete returning not-found has reached its desired state. The adapter honors acceptable
retry delays and surfaces permanent failures; redirects and transport-level automatic retries
cannot bypass the operation-specific policy.

The idempotent installs and downloads are
different: their motivating faults — a runner disk I/O error, a truncated fetch — are not reliably
classifiable from the error text, and the operations are cheap to repeat, so they retry *every*
failure within a small, bounded budget, and a genuinely deterministic failure (a bad version pin, a
checksum that never matches) costs only the capped backoff window before it surfaces.
Unavailable freshness evidence is qualified explicitly; it does not silently produce a
current-looking benchmark report.

First-party marketplace actions (checkout, cache, artifact up/download, and the like) are trusted
to retry their own network operations internally, so they are not wrapped in a third-party retry
action; adding one would trade the minimal-dependency stance for redundant coverage. That trust is
revisited only if a specific action is observed to flake.

## Job timeouts

Every job that runs `setup-environment` must budget for a *cold* cache. When the shared
dependency and toolchain caches miss — the warmup workflow lapses, a runner image rolls, or a
version pin changes — that step recompiles everything from scratch and can take up to ~90
minutes (which is why the warmup job, whose only work *is* that setup, is itself capped
generously). A job-level `timeout-minutes` bounds the whole job, setup included, so any
explicit cap is sized as the job's own work budget *plus* that ~90-minute cold-setup
allowance; sizing a cap to the warm-cache setup time alone would make a cache miss spuriously
fail the job. Jobs whose work is comfortably bounded carry no explicit cap and rely on
GitHub's default ceiling, which already clears a cold setup with room to spare. Explicit caps
exist only to stop a genuinely stuck run, never to bound the expected duration. The nightly
history backfill is the deliberate exception: its work is unbounded by nature (it keeps filling
gaps until it runs out of range), so it takes the ceiling as its run budget and pairs the cap
with `continue-on-error` so being cut off is an ordinary end rather than a failure.
