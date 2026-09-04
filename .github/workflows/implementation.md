# GitHub workflows implementation

This guide maps the workflow design to the repository tools that implement it. User-visible
CI behavior and design tenets are in [design.md](design.md); command flags and step details stay
with the commands and workflow jobs.

## Validation structure

`validation.yml` gives independently useful checks separate jobs so GitHub reports their
outcomes in parallel. Cargo-package jobs consume the affected-package set from the `delta`
job. Repository-wide checks run without that gate.

Pull requests and merge-queue entries use the pruned validation set. Pushes to `main` use the
full set. Queue delta analysis takes the event's base commit so its comparison cannot drift
from the queued merge candidate.

## Release validation

`cargo-release-plan` compares released content with version anchors and owns the report schema
and version-readiness verdict. The workflow passes the pull-request or merge-group base commit
as its release baseline.

`scripts/release/ReleasePlan.psm1` is the PowerShell boundary between that report and hosted
validation. It accepts only the report schema revision it understands and owns the explicit
set of packages whose library surface is a supported consumer contract. The same target
selection drives both the CI SemVer job and evidence collection by the increment-versions
skill. Package-name patterns do not determine whether a crate has a consumer contract.

The module also owns the skill's deterministic mechanics: dependency-order presentation,
publication eligibility, semantic change-decision validation, and conversion to
`cargo-release-plan apply` input. The just recipes remain thin command-line entry points.
Pester tests in `scripts/release/ReleasePlan.Tests.ps1` lock these boundaries.

On Windows, the module scopes `CARGO_TARGET_DIR` for direct `cargo-semver-checks` and
`release-plz update` invocations to a stable, workspace-specific directory beneath the user
temporary directory. This keeps the SemVer tool's nested placeholder builds independent of
checkout depth without changing target-directory behavior for unrelated Cargo commands or
non-Windows validation.

## Merge-blocking result

The `required-checks` job is the intended single ruleset target. Its `needs` graph contains
every merge-blocking Validation job. `scripts/build/RequiredChecks.psm1` rejects failed,
cancelled, missing, and unknown dependency results. It permits `skipped` only for jobs whose
event, platform, or package scope legitimately excludes them.

The classifier only observes what `needs` supplies, so it also rejects an unconditional gate
that its must-succeed list names but the payload omits. A name that drifts out of the `needs:`
list therefore fails the fan-in instead of silently disappearing from it.

Azure OIDC test jobs are among the legitimate queue skips because their federated identity
trusts pull-request and `main` subjects, not merge-group subjects.
