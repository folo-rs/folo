---
name: increment-versions
description: Propose and apply crate version increments for a pull request that changed released content. Use when the validate-versions check fails, when a pull request is ready to merge, or when the user asks to increment crate versions.
---

# Scope

An **increment** raises a package's version in `Cargo.toml` so unreleased changes become
pending release. Publishing is a separate process described in
[`RELEASING.md`](../../../RELEASING.md).

A **change level** describes the substance of a package's released changes:
`breaking`, `nonbreaking`, or `patch`. This skill decides change levels; it does not choose
version numbers. `cargo-release-plan apply` mechanically maps the approved levels to version
numbers, expands
[version groups](../../../packages/cargo-release-plan/README.md#plan-and-report-schema),
rewrites dependency requirements, and refreshes `Cargo.lock`.

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
| `base.txt` | Stage 2 | The pull request's base commit. |
| `report.json` | Stage 2 | The cargo-release-plan report. |
| `diffs/{{PACKAGE}}.patch` | Stage 2 | A package's released-content diff against its anchor. |
| `semver-checks.log` | Stage 2 | The console output of `cargo-semver-checks`. |
| `analysis-order.json` | Stage 3 | The analysis batches. |
| `decisions.json` | Stage 4 | The decided change levels. |
| `plan.json` | Stage 6 | The generated cargo-release-plan input. |

Commit none of them.

# Placeholders

| Placeholder | Description |
|-------------|-------------|
| `WORK_DIR` | The untracked directory holding this run's working files. |
| `VERIFY_DIR` | A second untracked directory, used only by Stage 7. |
| `DIFF_PATH` | A package's `diff_path` value from `report.json`. |
| `PACKAGE` | A package name. |
| `CHANGE_LEVEL` | A decided change level: `breaking`, `nonbreaking`, or `patch`. |

# Stage 1: Verify the SemVer checker

Prove that `cargo-semver-checks` can execute before using its output as evidence:

> just verify-semver-checks

Stop and report if the command exits non-zero. A checker that cannot execute must not be
interpreted as an absence of a required increment.

# Stage 2: Collect evidence

Record the pull request's base commit, then write the report, the per-package diffs, and the
SemVer evidence:

> gh pr view --json baseRefOid --jq .baseRefOid > "{{WORK_DIR}}/base.txt"
>
> $env:RELEASE_PLAN_BASE = Get-Content "{{WORK_DIR}}/base.txt"
>
> just release-report "{{WORK_DIR}}"

Stop and report if any command exits non-zero. `just release-report` accepts the documented
cargo-semver-checks finding exit and fails on every other non-zero exit, so a non-zero exit
here means the evidence is incomplete.

`base.txt` fixes the comparison base for the rest of the run. Every later `just release-report`
and `just validate-versions` invocation sets `RELEASE_PLAN_BASE` from it, because the default
base is the repository's default branch and a stacked pull request is not based on it.

[`report.json`](../../../packages/cargo-release-plan/README.md#plan-and-report-schema) lists
every publishable package with its `status`, `anchor`, `changed` array, `dependencies`, and
`diff_path`. A package has a `diff_path` when its released files differ from its anchor. One
whose `changed` entries are all `inherited` or `lockfile` has none, because neither is a file
difference.

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

`semver-checks.log` closes each checked package with one `Summary` line:

| Summary line | Floor |
|--------------|-------|
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

# Stage 5: Present the proposal

Present one row per package in `decisions.json`:

| Package | Change level |
|---------|--------------|
| `{{PACKAGE}}` | `{{CHANGE_LEVEL}}` |

Follow each row with its supporting explanation. The explanation may span multiple paragraphs
and must cite the `report.json` entry, diff path, or `semver-checks.log` summary it rests on.
State that the remaining analyzed packages need no increment. Explain that version-group
members will receive the maximum increment required by any member when the plan is applied.

Report separately, and outside that table, every `report.json` entry that has no `anchor`. Such
a package has never been published, so it needs a first publication as described in
[`RELEASING.md`](../../../RELEASING.md#first-publish-of-a-new-crate) rather than an increment.

Ask the caller to approve or adjust the change levels, and update `decisions.json` to match
what the caller approved. Do not continue until the caller approves them.

Stop here and report the approved change levels while the temporary rule in Scope applies.

# Stage 6: Apply approved changes

`cargo-release-plan apply` raises an existing version. It cannot create a crate on crates.io,
so a package that has never been published must first be published by hand as described in
[`RELEASING.md`](../../../RELEASING.md#first-publish-of-a-new-crate). Confirm that every
package the approved changes reach, directly or through a version group, is already published:

> just check-increment-published "{{WORK_DIR}}/report.json" "{{WORK_DIR}}/decisions.json"

Stop and report without generating or applying a plan if the command exits non-zero.

Generate the mechanical cargo-release-plan input:

> just create-release-plan "{{WORK_DIR}}/report.json" "{{WORK_DIR}}/decisions.json" "{{WORK_DIR}}/plan.json"

The command retains sufficient existing pending-release increments, raises insufficient ones,
and lets apply combine version-group decisions mechanically.

Apply the generated plan:

> just apply-release-plan "{{WORK_DIR}}/plan.json"

Stop and report if either command exits non-zero.

# Stage 7: Verify the result

Verify the lockfile, then collect fresh evidence for the resulting tree into a second untracked
directory:

> just verify-lockfile
>
> $env:RELEASE_PLAN_BASE = Get-Content "{{WORK_DIR}}/base.txt"
>
> just release-report "{{VERIFY_DIR}}"
>
> just validate-versions

Return to Stage 4 if any command exits non-zero, deciding again from the `{{VERIFY_DIR}}`
evidence: rerun Stage 3 against `{{VERIFY_DIR}}/report.json` first, because the applied
increments change the report.

`just release-report` exits zero on a SemVer finding, so read `{{VERIFY_DIR}}/semver-checks.log`
as well. Return to Stage 4 the same way if any package's `Summary` line now demands a level
above the one that was applied to it.

Commit the resulting `Cargo.toml`, dependency requirement, and `Cargo.lock` edits, and
summarize the approved package change levels in the pull request description.
