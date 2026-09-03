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

The repository's normal
[git workflow](../../../docs/git-workflow.md) keeps version increments off
feature branches. On a feature branch, proposal approval does not override that policy:
stop before Stage 7 unless the caller explicitly requested applying the increments on that
branch. On `main`, or when the caller explicitly directs a feature-branch exception, complete
the apply stages.

Do not commit the collected evidence, semantic change-decision file, or generated
cargo-release-plan input file.

# Stage 1: Verify the SemVer checker

Prove that `cargo-semver-checks` can execute before using its output as evidence:

> just verify-semver-checks

If the command reports an error, stop and report it. A checker that cannot execute must not be
interpreted as an absence of a required increment.

# Stage 2: Check first-publication status

List publishable packages that do not yet exist on crates.io:

> just check-never-published

If publication status cannot be confirmed, stop and report the affected package. Record any
never-published package for context; Stage 6 deterministically checks whether the approved
changes would reach it. First publication is the manual process in
[`RELEASING.md`](../../../RELEASING.md#first-publish-of-a-new-crate).

# Stage 3: Collect evidence

Resolve the pull request's base commit and collect the release-plan report, package diffs, and
SemVer evidence:

> $env:RELEASE_PLAN_BASE = gh pr view --json baseRefOid --jq .baseRefOid
>
> just release-report "{{OUT_DIR}}"

Placeholders to replace:

| Placeholder | Description |
|-------------|-------------|
| `OUT_DIR` | An untracked working directory for `report.json`, `diffs/`, and `semver-checks.log`. |

If either command reports an error, stop and report it. The collection recipe accepts the
documented cargo-semver-checks finding exit, but rejects operational failures.

Use the collected files only for the current work-tree content. If published content changes,
repeat this stage before presenting or applying decisions.

# Stage 4: Determine analysis order

Read package dependencies from the collected report and print the deterministic analysis
order:

> just release-analysis-order "{{REPORT}}"

Placeholders to replace:

| Placeholder | Description |
|-------------|-------------|
| `REPORT` | `{{OUT_DIR}}/report.json` from Stage 3. |

If the command reports an error, stop and report it. Analyze batches in the printed order.
Packages in one cyclic batch depend on each other; analyze them in the printed order and
repeat that batch until no decision changes.

# Stage 5: Decide change levels

Analyze every package in the report, including `pending-release` and `unchanged` packages.
For each package, inspect its own released-content evidence, its `Cargo.toml`, fields it
inherits from the workspace `Cargo.toml`, and changes to dependencies already analyzed.
Inherited fields appear as `source: "inherited"` entries in `report.json` and may have no
package diff. A dependency decision can make a dependent require an increment even when the
dependent had no original file change.

Use [determining-level.md](determining-level.md) to choose `breaking`, `nonbreaking`, `patch`,
or no increment. Treat a cargo-semver-checks finding as a `breaking` floor. When
cargo-semver-checks could not determine a required version increment, it supplied no floor;
its result is not evidence that no increment is required. Analyze `pending-release` packages
fully and raise their existing increment when the newly decided change level requires it.
Never lower an increment already present in `Cargo.toml`.

Present only packages that need an increment:

| Package | Change level |
|---------|--------------|
| `{{PACKAGE}}` | `{{CHANGE_LEVEL}}` |

Follow each proposed package with its supporting explanation. The explanation may span
multiple paragraphs and must cite the relevant report entry or diff path. State that omitted
packages were analyzed and need no increment. Explain that version-group members will receive
the maximum increment required by any member when the plan is applied.

Ask the caller to approve or adjust the change levels. Do not continue until the caller
approves them.

# Stage 6: Check approved changes

Write the approved decisions to an untracked JSON file:

```json
{
  "schema_version": 1,
  "changes": [
    { "name": "nm", "level": "breaking" },
    { "name": "events", "level": "patch" }
  ]
}
```

Omit packages that need no increment. Only `breaking`, `nonbreaking`, and `patch` are valid
change levels.

Confirm that every package reached directly or through a version group has already been
published:

> just check-increment-published "{{REPORT}}" "{{DECISIONS}}"

Placeholders to replace:

| Placeholder | Description |
|-------------|-------------|
| `REPORT` | `{{OUT_DIR}}/report.json` from Stage 3. |
| `DECISIONS` | The approved semantic change-decision JSON file. |

If the command reports a never-published package or cannot confirm publication, stop and
report the package. Do not generate or apply a plan.

# Stage 7: Apply approved changes

If the current branch is proposal-only under the Scope rules, stop after reporting the
approved change levels.

Generate the mechanical cargo-release-plan input:

> just create-release-plan "{{REPORT}}" "{{DECISIONS}}" "{{PLAN}}"

Placeholders to replace:

| Placeholder | Description |
|-------------|-------------|
| `REPORT` | `{{OUT_DIR}}/report.json` from Stage 3. |
| `DECISIONS` | The approved semantic change-decision JSON file from Stage 6. |
| `PLAN` | An untracked path that will receive cargo-release-plan's input file. |

The command retains sufficient existing pending-release increments, raises insufficient ones,
and lets apply combine version-group decisions mechanically. If it reports an error, stop and
report it.

Apply the generated plan:

> just apply-release-plan "{{PLAN}}"

Placeholders to replace:

| Placeholder | Description |
|-------------|-------------|
| `PLAN` | The generated cargo-release-plan input file. |

If apply reports an error, stop and report it.

# Stage 8: Verify the result

Verify the lockfile and version state:

> just verify-lockfile
>
> just validate-versions

If either command reports an error, stop and return to Stage 5 with the new evidence.

Collect fresh SemVer evidence for the resulting tree:

> just release-report "{{VERIFY_DIR}}"

Placeholders to replace:

| Placeholder | Description |
|-------------|-------------|
| `VERIFY_DIR` | A new untracked working directory for verification evidence. |

If the command reports an error, stop and report it. Do not commit the evidence,
change-decision, or plan files. Commit the resulting `Cargo.toml`, dependency requirement, and
`Cargo.lock` edits. Summarize the approved package change levels in the pull request
description.
