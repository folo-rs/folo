# GitHub workflows design

The high-level design of this repository's CI/CD workflows: the patterns they share,
the tenets behind them, and how the pieces relate. Per-job mechanics live in inline
YAML comments and in the `just` recipes the steps call; this document stays high-level.

## Job granularity and gating

Validation runs each `just` command as its own parallel job rather than one combined
`validate-local` step. Parallelism gives faster feedback and pinpoints failures by check
name instead of burying them in a monolithic log. Expensive jobs gate behind cheaper
equivalents so a fast failure short-circuits slow work — for example, Miri and mutation
testing only start once the plain dev `cargo check` and the base test pass have already
succeeded, since there is nothing to interpret or mutate in code that does not compile or
whose tests already fail.

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

## Concurrency

Commit-driven and PR-driven workflows cancel superseded runs, keyed on the ref, so pushing
a new commit abandons the outdated run. The exception is history collection, which is keyed
on the commit **SHA**: each commit is a distinct measurement, so distinct commits must run
in parallel and only a redundant re-trigger of the *same* commit is deduplicated.
Schedule-driven workflows carry no concurrency block at all.

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
A downstream analysis job reads the accumulated
history and files a single rolling, advisory issue when it detects a notable regression;
regressions never fail the run. Because a GitHub issue body is size-capped and a large
analysis can exceed it, the issue carries a **condensed summary** (the top findings) and
links to the **full Markdown and JSON reports**, which the job uploads as a run artifact — so
the issue always fits while the complete data stays one click away.

## Failure alerting

The history and release workflows both open a GitHub issue on failure, but with
deliberately different lifecycles matched to what failed. A benchmark-history failure is
a recurring condition on a rolling target, so it opens a *deduplicated* tracking issue
(keyed on a fixed title) that a companion job closes automatically once the workflow is
green again — exactly one open issue per persistent failure, cleared without manual
intervention. A release failure is a discrete event tied to one publish attempt, so it
opens a *per-run* issue (identified by the failing run) that stays open until a human
investigates; each failed release is tracked individually rather than folded into a
rolling issue.

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

