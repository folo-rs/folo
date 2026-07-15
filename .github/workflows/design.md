# GitHub workflows design

The high-level design of this repository's CI/CD workflows: the patterns they share,
the tenets behind them, and how the pieces relate. Per-job mechanics live in inline
YAML comments and in the `just` recipes the steps call; this document stays high-level.

## Job granularity and gating

Validation runs each `just` command as its own parallel job rather than one combined
`validate-local` step. Parallelism gives faster feedback and pinpoints failures by check
name instead of burying them in a monolithic log. Expensive jobs gate behind cheaper
equivalents so a fast failure short-circuits slow work — for example, Miri and mutation
testing only start once the dev Clippy pass and the base test pass have already succeeded,
since there is nothing to interpret or mutate in code that does not compile or whose tests
already fail. Clippy stands in for a bare `cargo check` here: Clippy compiles the code as a
prerequisite to linting it, so a standalone `check` job would only re-prove what a green
Clippy already guarantees.

## Selective validation

Most jobs are package-scoped and skip packages a change does not touch: a `delta` job
computes the affected set and downstream jobs consult it, so a one-package PR does not
rebuild the workspace. The complement of this pattern is the rule that a job whose inputs
are **not** Cargo packages — the workflow files themselves, or the standalone PowerShell
under `scripts/` — must run unconditionally. Delta analysis reports "nothing affected" for
such a change, so gating those jobs on it would leave the change validated by nothing.

## Platform strategy

Test passes are organised as an x86_64/ARM64 pair. The x64 pass carries coverage
instrumentation (which needs a nightly-only toolchain component), while the ARM pass
doubles as the MSRV pass and exists to exercise architecture-gated code that x86_64 runners
never compile. macOS is Apple Silicon, so it rides the ARM pass. Miri follows the same
shape: it is an architecture-agnostic interpreter, so a second ARM run earns its keep only
by subjecting ARM-gated paths to Miri's UB detection. Platform-agnostic checks (formatting,
workflow validation, script tests) run on a single Linux runner because their result cannot
vary by platform.

Not every check earns its place on every pull request. The full matrix runs on each push to
`main`, but pull-request validation prunes the rarely-informative legs to cut runner cost,
leaning on push-to-`main` as the backstop for what it drops. PRs run the test and docs
suites only on the x86_64 Windows and Linux runners: the whole ARM pass (which carries the
MSRV *test* run) and the macOS legs of the test and docs jobs wait for `main`, because
architecture- and OS-gated behaviour rarely diverges on a PR and re-running the
platform-independent test and doc suites on macOS almost never is informative. The base Miri
pass runs on Windows only for a PR — being an architecture-agnostic interpreter, its Linux
and ARM re-runs are a `main`-only sanity net over cfg-gated paths — and the release-profile
Clippy pass and the `careful` run are skipped entirely on PRs. The many-seeds Miri passes are
the exception to that pruning: gated by *package* rather than by event, they run on Linux —
on a PR as much as on `main` — whenever their specific package is touched, because their
worth is catching seed-dependent UB in that code, not covering a platform. The
compile-oriented passes (dev Clippy, release build, frozen-minimum check, feature `hack`)
deliberately keep their macOS leg on PRs, because a cheap macOS cross-compile still catches
macOS-specific build breaks that the pruned runtime passes would not. MSRV *compilation*
therefore stays covered on every PR by `check-frozen`, which compiles all targets on the
MSRV toolchain against the frozen minimum-version lockfile even though the ARM MSRV test
pass is `main`-only. Because a push to `main` is the first place the pruned checks can fail,
that event — unlike a PR — files a tracking issue (see Failure alerting).

The event split is expressed two ways: a job whose every leg is pruned on a PR (the ARM test
and Miri passes, `clippy-release`, `careful`) carries a whole-job `github.event_name ==
'push'` guard, while a job that keeps some legs on a PR (macOS-dropping test/docs, the
Ubuntu-dropping `miri-x64`) selects its platform list with a `fromJSON` conditional matrix
keyed on the same event. Both reduce to "the full set on push, the pruned set on a PR".

## Concurrency

Commit-driven and PR-driven workflows cancel superseded runs, keyed on the ref, so pushing
a new commit abandons the outdated run. That supersession only fires when a *new commit*
arrives on the branch, so closing or merging a PR — which pushes nothing to the PR branch —
would otherwise leave its in-flight Validation run to burn to completion. A dedicated
companion workflow closes that gap: it triggers on the PR-close event and joins the target
workflow's concurrency group so cancel-in-progress reclaims the stale run. Both the Validation
workflow and the PR benchmark-history workflow pair with such a close companion. The exception
is history collection on `main`, which is keyed on the commit **SHA**: each commit is a distinct
measurement, so distinct commits must run in parallel and only a redundant re-trigger of the
*same* commit is deduplicated. Schedule-driven workflows carry no concurrency block at all.

## Thin steps

Workflow steps stay thin. Non-trivial logic lives in PowerShell `[script]` `just` recipes
the steps call, so it runs and is debugged locally instead of only by pushing to `main`.
Logic worth unit-testing goes one level deeper into a module under `scripts/` covered by a
Pester suite. Every `run:` step uses `pwsh`; the `setup-environment` composite is the sole
Bash holdout because it bootstraps PowerShell itself.

## Coverage reporting

Coverage is a side effect of the ordinary test run, not a separate re-execution. A single
commit therefore produces several coverage uploads — one per platform, plus a conditional
upload from the Azure-backend job. Codecov is configured to hold all notifications until a
final gate job signals that every expected upload for the commit has landed, so the reported
figure is computed from the complete set rather than flapping as partial uploads arrive. The
gate keys off "every expected upload succeeded or was legitimately skipped", never off a
hardcoded upload count, because the Azure upload is conditional.

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
runs, since a fork cannot federate into the tenant.

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
standard `AZURE_*` names the tool and `azure/login` read (`AZURE_PROD_CLIENT_ID` →
`AZURE_CLIENT_ID`) by a single shared federation step, so the mapping is defined once rather
than copy-pasted per job.

Federation only works for **same-repo** runs: a fork's run cannot mint a token whose subject
names this repository, so fork PRs skip the Azure-touching jobs rather than fail.

Two managed identities exist, each registered with exactly the subjects its events present:

| Event | OIDC subject | Identity | Consumer |
| --- | --- | --- | --- |
| push to `main` | `…:ref:refs/heads/main` | prod | `bench-history.yml` |
| pull request | `…:pull_request` | prod | `pr-bench-history.yml` |
| push to `main` | `…:ref:refs/heads/main` | test | `test-azure` backend tests |
| pull request | `…:pull_request` | test | `test-azure` backend tests |

The **prod** identity backs history collection and the PR benchmark workflow; the **test**
identity backs the Azure-backend test jobs against a throwaway account. Both trust `main` and
`pull_request` so each identity's on-main and on-PR consumers can sign in. Granting the prod
identity a `pull_request` credential is a deliberate tradeoff: it widens prod's write surface
from "only pushes to `main`" to "any same-repo PR run", accepting a larger blast radius in
exchange for letting a PR's benchmarks be compared against the very store that holds `main`'s
baseline. The narrower alternative — a separate PR store — was rejected because branch-mode
analysis must read the base's accumulated history and the tool reads a single backend.

## Benchmark history

History collection runs on every push to `main` rather than on a schedule, so it captures
exactly one data point per commit instead of re-measuring an unchanged tip and missing
intermediate commits. It writes to a dedicated production storage account under a dedicated
production managed identity, kept entirely separate from the throwaway account the test jobs
use, so the long-lived data store never depends on test infrastructure. Collection is
append-only and idempotent, which is what makes a re-run safe and lets a read-through cache
of the bulk history persist between runs. Collection stamps the wall-clock (per-machine)
engines with a fixed `github` machine key instead of the auto-detected hardware fingerprint,
and analysis reads only that key: the GitHub-hosted runner pool may hold differently-specced
machines, so fingerprinting would fork the series on every SKU change and let unrelated
machines pollute the data set, whereas one fixed key keeps a single series (accepting the
pool's jitter) that a deliberate blessing absorbs across a genuine hardware migration.
To blunt that jitter rather than merely accept it, collection runs the whole suite several
times per commit and keeps, per metric, the minimum sample: runner interference is one-sided
(a contended host only ever makes a benchmark slower) and the repeats are spaced apart in
time, so the minimum is the reading least perturbed by transient noise. This trades a
proportionally longer collection job for a more stable series.

Even with those defences, a single day of a badly degraded runner can still leave one commit's
data point corrupted. Collection is therefore also manually re-runnable against a specific
historical commit: a `workflow_dispatch` with a `recollect_commit_id` re-measures just that
commit and *overwrites* its stored point instead of appending the pushed tip. The subtlety this
resolves is that the collection tool lives in the same repository as the benchmarks, so a naive
"check out that commit and re-run" would also run the tool as it shipped at that commit. Instead
the re-collection benchmarks the code *at* the target commit in a throwaway worktree while
running the current tool, so only the measured code — never the collection logic — comes from
the past. The overwrite bumps the cache-invalidation marker so downstream analysis refreshes,
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
history and files a single rolling, advisory issue when it detects a notable regression;
regressions never fail the run. Because a GitHub issue body is size-capped and a large
analysis can exceed it, the issue carries a **condensed summary** (the top findings) and
links to the **full Markdown and JSON reports**, which the job uploads as a run artifact — so
the issue always fits while the complete data stays one click away.

### PR benchmark history

The same measure-and-report loop runs on pull requests, retuned to answer "does this PR move
any benchmark relative to `main`?" instead of "is `main` trending?". It reuses the analysis
tool's **branch mode**: with the PR head as context and `main` as the base, the tool splits the
head's first-parent ancestry at the merge-base and compares the branch tip's level against the
base ancestry's clean baseline, so a finding means *this branch changed the level* rather than
that the long-range trend moved. To keep that topology intact the collect and analyze jobs
check out the PR head's real commit — not the synthetic `pull_request` merge ref, whose first
parent is `main` and would corrupt the comparison — with full history, since both the
merge-base and the first-parent walk need it.

Because a PR is transient, the findings land in a single **rolling PR comment** (deduped by a
hidden marker, updated in place on every push) rather than the rolling issue the `main`
workflow files; the comment lives and dies with the pull request. The comment is strictly
advisory — findings never affect the check's exit code, so a regression note never blocks a
merge — and it reports detected improvements alongside regressions, or a plain "no regressions"
state. It also states its **collection scope** — which packages were benchmarked — so a clean
result is never mistaken for the whole suite being clean when only the changed subset was
measured. A run *failure* surfaces only as the red check, with no issue and no failure comment,
because a PR failure is a transient condition, not the persistent one the issue lifecycle
tracks.

Because a full benchmark run takes hours and a new push *cancels* the in-flight one (see
Concurrency), on a PR's first push there is nothing on display yet, and on later pushes the comment
on display can lag the PR tip by a long way with no way for a reader to tell current numbers from
hours-old ones. A lightweight **`mark-stale` job** runs at the *start* of each new run (right after
the short delta preflight, in parallel with the multi-hour collect) and keeps the comment honest
about the run just begun. When the PR has **no comment yet**, it seeds a *"benchmarking in
progress"* placeholder — carrying the same hidden dedup marker and disclosing the collection scope,
so the author knows results are coming rather than seeing nothing for hours; it refreshes that
placeholder's scope on later pushes and steps aside once a completed analyze overwrites it with real
findings. When a comment **already carries results**, two further mechanisms flag their age: every
such comment records **which commit it measured** — a human-visible line printing the full SHA bare
(GitHub autolinks it to the commit and abbreviates it for display, so we neither truncate it
ourselves nor lose the click-through) plus a hidden full-SHA marker — and `mark-stale` prepends a
warning banner stating how far behind `HEAD` those numbers now are: *"N commits behind HEAD"*, or a
numberless *"out of date"* when the two share no history (e.g. a force-push) or the marker is
absent. The distance comes from the GitHub compare API (`ahead_by`), which needs no clone and still
resolves a commit orphaned by a force-push; any inability to compute it degrades to the numberless
wording rather than failing the run. The banner is bounded by a sentinel pair so a re-run *replaces*
rather than stacks it, and the next completed analyze — which rewrites the body from scratch with a
fresh analyzed-commit marker — drops it automatically once real new results land. The staleness pass
skips a still-empty placeholder (it has no results to age), leaving the placeholder's own upkeep to
the seeding pass. `mark-stale` is gated on the same non-empty delta as collect, so it never races the
cleanup path that *deletes* the comment when the PR no longer touches anything benchmarkable.

Collection is **delta-scoped**: a preflight job diffs the PR against `main` and benchmarks only
the touched packages, since re-measuring the whole workspace on every PR push would be
wasteful. Analysis, by contrast, is deliberately **not** package-scoped — yet it stays correctly
scoped anyway, by construction rather than by a name filter. `analyze` by default considers only
benchmarks **present at the context commit** (the PR head), dropping any "ghost" benchmark with
no run there *before* detection. Because only the touched packages are collected at the PR's
branch-unique head commit, only they are present there, so every untouched package is excluded
as a ghost automatically — for *every* measurement engine — leaving exactly the collected set to
analyze. Package scoping thus falls out of *what gets collected*, with no need to filter analysis
by name, which would in fact be wrong: benchmark identities are engine-dependent (some engines
identify a series by bare operation name with no package prefix), so a name filter would silently
drop those series and turn a real regression into a false negative. The PR analysis therefore
relies on that ghost exclusion, which is unconditional and cannot be turned off. As a side
benefit the same filter
also drops a benchmark the PR itself *removed*, so a deletion is never mis-reported as a
regression.

When a PR touches no benchmarkable package — including a PR that touched one earlier and then
reverted it — a lightweight cleanup path removes any rolling comment a prior push left behind
and posts nothing, so a stale, misleading comment never lingers; it is a no-op when there was
no comment. PR runs read the shared history cache **restore-only** (never saving), keeping the
baseline warm without accumulating per-PR cache entries, which the append-only store makes
safe even when slightly stale.

## Failure alerting

The history, release, and validation workflows all open a GitHub issue on failure, but with
deliberately different lifecycles matched to what failed. A benchmark-history failure is
a recurring condition on a rolling target, so it opens a *deduplicated* tracking issue
(keyed on a fixed title) that a companion job closes automatically once the workflow is
green again — exactly one open issue per persistent failure, cleared without manual
intervention. A release failure is a discrete event tied to one publish attempt, so it
opens a *per-run* issue (identified by the failing run) that stays open until a human
investigates; each failed release is tracked individually rather than folded into a
rolling issue. A push-to-`main` Validation failure follows the same per-run shape as the
release alert — a fresh `ci-failure` issue per failing run, no dedup and no auto-close —
because it now backstops the checks pruned from PR validation, so each such failure warrants
individual triage. It fires *only* on push to `main`: a PR failure is already self-evident as
the red check and needs no issue, so the alert is gated on the `main` ref (which a
`pull_request` run never presents) and on `failure()`, leaving a green or skipped-only run to
file nothing.

## Release automation

Publishing changed crates to crates.io and attaching cargo-binstall prebuilt binaries is
fully automated after the single manual version-bump step. Its full design — single-workflow
structure, crates.io Trusted Publishing, dynamic derivation of which crates receive GitHub
releases, and the self-healing reconciliation that rebuilds only missing binary assets —
lives in [`docs/release-automation.md`](../../docs/release-automation.md).

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
exist only to stop a genuinely stuck run, never to bound the expected duration.

