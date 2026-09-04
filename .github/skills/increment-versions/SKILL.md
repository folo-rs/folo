---
name: increment-versions
description: Propose and apply crate version increments for a pull request that changed released content. Use when the validate-versions check fails, when a pull request is ready to merge, or when the user asks to increment crate versions.
---

# Scope

An **increment** raises a package's version in `Cargo.toml` so unreleased changes become
pending release. Publishing is a separate process described in
[`RELEASING.md`](../../../RELEASING.md).

A **change level** describes the substance of a package's released changes:
`breaking`, `nonbreaking`, or `patch`. A fourth decision, `align`, records that a version
group's members must agree on a version without any of them having changed. This skill decides
change levels; it does not choose version numbers. `cargo-release-plan` maps the approved levels
to version numbers and expands
[version groups](../../../packages/cargo-release-plan/README.md#plan-and-report-schema) so a
group's members stay on one version, then rewrites dependency requirements and refreshes
`Cargo.lock`.

This skill applies to a feature branch. Confirm the branch before Stage 1:

> git rev-parse --abbrev-ref HEAD

Stop and report if that prints `main`.

**Temporary rule.** [`docs/git-workflow.md`](../../../docs/git-workflow.md) keeps version
increments off feature branches. While it does, a run stops after Stage 5 and reports the
approved change levels. Stages 6 and 7 apply once that rule permits increments inside a pull
request.

# Working files

Every stage reads and writes files under `{{WORK_DIR}}`, one untracked directory chosen for the
run. Write each command's output to its file as the command runs rather than reconstructing it
afterwards, and read a later stage's inputs from these files rather than from memory.

| File | Written in | Contents |
|------|------------|----------|
| `base.txt` | Stage 2 | The release baseline commit. |
| `report.json` | Stage 2 | The cargo-release-plan report. |
| `diffs/{{PACKAGE}}.patch` | Stage 2 | A package's released-content diff against its anchor. |
| `semver-checks.log` | Stage 2 | The console output of `cargo-semver-checks`. |
| `analysis-order.json` | Stage 3 | The analysis batches. |
| `decisions.json` | Stage 4 | The decided change levels. |
| `plan.json` | Stage 5 | The generated cargo-release-plan input. |
| `expanded.json` | Stage 5 | That plan with version groups resolved into per-package versions. |

Commit none of them.

# Placeholders

| Placeholder | Description |
|-------------|-------------|
| `WORK_DIR` | The untracked directory holding this run's working files. |
| `VERIFY_DIR` | A second untracked directory, written by Stage 7 and adopted as `WORK_DIR` when Stage 7 sends the run back to Stage 4. |
| `DIFF_PATH` | A package's `diff_path` value from `report.json`. |
| `PACKAGE` | A package name. |
| `CHANGE_LEVEL` | A decided change level: `breaking`, `nonbreaking`, `patch`, or `align`. |
| `NEW_VERSION` | A package's resolved version from `expanded.json`. |

# Stage 1: Verify the SemVer checker

Prove that `cargo-semver-checks` can execute before using its output as evidence:

> just verify-semver-checks

Stop and report if the command exits non-zero. A checker that cannot execute must not be
interpreted as an absence of a required increment.

# Stage 2: Collect evidence

Create the directory, record the release baseline, then write the report, the per-package diffs,
and the SemVer evidence:

> New-Item -ItemType Directory -Force -Path "{{WORK_DIR}}"
>
> git fetch origin main
>
> git rev-parse FETCH_HEAD > "{{WORK_DIR}}/base.txt"
>
> $env:RELEASE_PLAN_BASE = Get-Content "{{WORK_DIR}}/base.txt"; just release-report "{{WORK_DIR}}"

Stop and report if any command exits non-zero. `just release-report` accepts the documented
cargo-semver-checks finding exit and fails on every other non-zero exit, so a non-zero exit
here means the evidence is incomplete.

The baseline is the tip of the branch releases are made from, not the branch this pull request
targets. A stacked pull request targets an unreleased parent branch, and anchoring on it would
read that parent's pending increment as a release and hide the parent's unreleased changes. The
fetch is what makes the baseline current: a local remote-tracking ref can lag behind the release
branch, which would present an already-released increment as still pending.

`base.txt` fixes that baseline for the rest of the run, so every later `just release-report` and
`just validate-versions` invocation sets `RELEASE_PLAN_BASE` from it and no stage silently
compares against a different revision. An environment variable does not outlive the shell that
set it, so set it in the same invocation as the command that reads it.

[`report.json`](../../../packages/cargo-release-plan/README.md#plan-and-report-schema) lists
every publishable package with its `status`, `anchor`, `changed` array, `dependencies`, and
`diff_path`. A package has a `diff_path` when its released files differ from its anchor. One
whose `changed` entries are all `inherited` or `lockfile` has none, because neither is a file
difference.

Its `groups` object names each version group's `members` and whether they currently declare one
version, in `consistent`. `just validate-versions` fails on an inconsistent group as well as on
a package needing an increment, so both are conditions this skill resolves.

The files describe the current work-tree content. Repeat this stage if that content changes
before the decisions are presented or applied.

# Stage 3: Determine analysis order

Write the dependency-first analysis batches:

> just release-analysis-order "{{WORK_DIR}}/report.json" > "{{WORK_DIR}}/analysis-order.json"

Stop and report if the command exits non-zero.

`analysis-order.json` is a JSON array. Every package in `report.json` appears in exactly one
batch:

```json
[
  { "order": 1, "packages": ["events"], "cyclic": false },
  { "order": 2, "packages": ["nm", "nm_impl"], "cyclic": true }
]
```

| Field | Meaning |
|-------|---------|
| `order` | Ascending analysis position. Every dependency of a batch sits in a lower `order`. |
| `packages` | The batch members, in analysis order. |
| `cyclic` | `true` when the members depend on each other. |

# Stage 4: Decide change levels

Work through `analysis-order.json` in ascending `order`, deciding every package in the batch
before moving to the next one. This order guarantees that a package's workspace dependencies
are already decided when the package itself is decided. Repeat a `cyclic` batch until none of
its entries change.

Decide each package from these inputs:

* its entry in `report.json`, including `status` and the `changed` array;
* the diff at `{{WORK_DIR}}/{{DIFF_PATH}}` when the entry has a `diff_path`;
* the package's `Cargo.toml` and the workspace `Cargo.toml` fields it inherits;
* the entries already recorded in `decisions.json` for the packages it depends on; and
* the package's floor in `semver-checks.log`.

A group that `report.json` marks `"consistent": false` needs a decision even when no member's
released content changed. Its members declare different versions, which is a check failure in
its own right. Expansion realigns a group only when a decision names one of its members, so
without a decision here nothing acts on the group and the check stays red.

Decide such a group at the level its members' accumulated changes justify. When they justify
nothing, decide `align`: the members then move to the highest version any of them already
declares, which is what they must agree on. A change level would instead raise that highest
version, publishing a new release of every member for no substantive change.

`semver-checks.log` closes each checked package's block with one `Summary` line. The package is
the one named in the `Checking` line that opens the block. The table lists line prefixes: a
summary that demands an increment continues with a count of the checks that failed.

| Summary line prefix | Floor |
|---------------------|-------|
| `Summary no semver update required` | None. |
| `Summary semver requires new minor version` | `nonbreaking`. |
| `Summary semver requires new major version` | `breaking`. |

A package absent from the log has no floor. An absent floor is not evidence that no increment
is required, because `cargo-semver-checks` inspects only part of the Rust API surface. Raise a
decision to at least its floor and never below it, and never below an increment already
declared in the package's `Cargo.toml`.

Use [determining-level.md](determining-level.md) to choose `breaking`, `nonbreaking`, `patch`,
or no increment.

Append each batch's outcome to `decisions.json` before starting the next batch, so the next
batch reads its dependency decisions from the file. Omit a package that needs no increment.

```json
{
  "schema_version": 1,
  "changes": [
    { "name": "nm", "level": "breaking" },
    { "name": "events", "level": "patch" }
  ]
}
```

# Stage 5: Resolve version groups and present the proposal

A version group's members share one version, so a decision for any member moves all of them.
Resolve that before asking for approval, so the caller sees every package the decision moves and
the version each will carry rather than a set that widens during apply.

Generate the mechanical cargo-release-plan input, then resolve its groups:

> just create-release-plan "{{WORK_DIR}}/report.json" "{{WORK_DIR}}/decisions.json" "{{WORK_DIR}}/plan.json"
>
> just expand-release-plan "{{WORK_DIR}}/plan.json" "{{WORK_DIR}}/expanded.json"

Stop and report if either command exits non-zero. `create-release-plan` retains sufficient
existing pending-release increments and raises insufficient ones. `expand-release-plan` applies
each group's highest decided level to the highest version any of its members declares, and names
every member at the resulting version.

`expanded.json` is a cargo-release-plan input whose every entry carries an explicit `version`:

```json
{
  "schema_version": 1,
  "increments": [
    { "name": "nm", "version": "2.0.0" },
    { "name": "nm_impl", "version": "2.0.0" }
  ]
}
```

Present one row per version group, and one per ungrouped package, reading the members from
`report.json` and the versions from `expanded.json`:

| Packages | Change level | New version |
|----------|--------------|-------------|
| `{{PACKAGE}}` | `{{CHANGE_LEVEL}}` | `{{NEW_VERSION}}` |

A group's row lists every member and the level that governs the group, which is the highest
level decided for any of its members. Follow each row with its supporting explanation. The
explanation may span multiple paragraphs and must cite the `report.json` entry, diff path, or
`semver-checks.log` summary it rests on. Name any member that is moving only because it shares a
group. State that the remaining analyzed packages need no increment.

Report separately, and outside that table, every `report.json` entry that has no `anchor`. Such
a package has never been published, so it needs a first publication as described in
[`RELEASING.md`](../../../RELEASING.md#first-publish-of-a-new-crate) rather than an increment.

Ask the caller to approve or adjust the change levels. An adjustment is still bound by the
floors in Stage 4: report the conflict rather than recording a level below one. To record an
adjustment, edit `decisions.json` and repeat this stage from `create-release-plan`. Never edit
`plan.json` or `expanded.json` directly; a hand-edited expansion can give one group's members
different versions, which the tool rejects. Do not continue until the caller approves.

Stop here and report the approved change levels while the temporary rule in Scope applies.

# Stage 6: Apply approved changes

`cargo-release-plan apply` raises an existing version. It cannot create a crate on crates.io,
so a package that has never been published must first be published by hand as described in
[`RELEASING.md`](../../../RELEASING.md#first-publish-of-a-new-crate). Confirm that every
package the approved expansion names is already published:

> just check-increment-published "{{WORK_DIR}}/expanded.json"

Stop and report without applying anything if the command exits non-zero.

Apply the approved expansion, which is the document the caller saw:

> just apply-release-plan "{{WORK_DIR}}/expanded.json"

Stop and report if the command exits non-zero.

# Stage 7: Verify the result

Verify the lockfile, then collect fresh evidence for the resulting tree into a second untracked
directory:

> just verify-lockfile
>
> New-Item -ItemType Directory -Force -Path "{{VERIFY_DIR}}"
>
> Copy-Item "{{WORK_DIR}}/base.txt" "{{VERIFY_DIR}}/base.txt"
>
> $env:RELEASE_PLAN_BASE = Get-Content "{{VERIFY_DIR}}/base.txt"; just release-report "{{VERIFY_DIR}}"
>
> $env:RELEASE_PLAN_BASE = Get-Content "{{VERIFY_DIR}}/base.txt"; just validate-versions

Stop and report if `just verify-lockfile` exits non-zero. Refreshing the lockfile is part of
applying a plan, so a stale lockfile here is a defect to report rather than a decision to revisit.

Return to Stage 4 if either later command exits non-zero. Before doing so, adopt
`{{VERIFY_DIR}}` as the run's `{{WORK_DIR}}`: the applied increments changed the report, and
every later stage reads its inputs from `{{WORK_DIR}}`, so leaving the old directory in place
would regenerate a plan from pre-apply evidence and increment the same packages a second time.
The copied `base.txt` keeps the adopted directory on the original baseline. Rerun Stage 3
against the adopted directory's `report.json` first.

`just release-report` exits zero on a SemVer finding, so read `{{VERIFY_DIR}}/semver-checks.log`
as well. Return to Stage 4 the same way if any package's `Summary` line now demands a level
above the one that was applied to it.

Commit the resulting `Cargo.toml`, dependency requirement, and `Cargo.lock` edits, and
summarize the approved package change levels in the pull request description.
