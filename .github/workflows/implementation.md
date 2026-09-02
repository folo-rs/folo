# GitHub workflows implementation

Ownership boundaries for the Validation release-check pipeline. User-visible
behavior is in [design.md](design.md). Command-line flags and step scripts stay
beside the jobs and recipes that run them.

## Data flow

```text
event SHA  -->  RELEASE_PLAN_BASE / DELTA_BASELINE
                |                    |
                v                    v
     cargo-release-plan          cargo-delta
     classify (Rust)             affected packages
                |
                +--> check --format github  -->  validate-versions verdict
                |
                +--> report.json
                        |
                        v
                 ReleasePlan.psm1
                 (public-shell filter)
                        |
                        v
                 released=   -->  just semver-checks
                                          |
                                          v
                 needs results  -->  RequiredChecks.psm1  -->  fan-in
```

## Ownership

* **Release baseline.** `validation.yml` selects `github.event.pull_request.base.sha`
  or `github.event.merge_group.base_sha` as `RELEASE_PLAN_BASE`. An empty value on
  push to `main` leaves the tool default. The same SHA is `DELTA_BASELINE` on a
  queue run so delta and the version check cannot drift.
* **Classification.** `cargo-release-plan` owns workspace classification, report
  schema, and the check verdict. Recipes do not reimplement it. The report schema
  is documented in `packages/cargo-release-plan/README.md`.
* **Public-package selection.** `scripts/release/ReleasePlan.psm1` reads
  `report.json` and selects consumer-visible comparison targets. Locked by
  `scripts/release/ReleasePlan.Tests.ps1`.
* **Valid-empty skip.** An empty `released` output is a present empty string. The
  `semver-checks` recipe treats that as a successful skip, not a workspace-wide
  run.
* **Required-result classification.** `scripts/build/RequiredChecks.psm1` reads
  `toJSON(needs)` and the must-succeed job list from the fan-in step. Locked by
  `scripts/build/RequiredChecks.Tests.ps1`.
