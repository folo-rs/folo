---
name: increment-versions
description: Assess and apply complete Cargo workspace version increments using cargo-release-plan. Use when version validation fails, a pull request is ready for review, or the user requests version increments.
---

# Scope

Assess released changes, choose semantic levels, preview their complete dependency effects,
and apply the resolved version plan. Human review of the complete contribution is the approval
step; do not add a separate approval pause for version choices. This skill authorizes neither
merging nor registry publication, tag creation, or Trusted Publisher administration.

This directory is self-contained. Copy the whole directory, including its decision guide and
license. The [public user guide](https://folo-rs.github.io/folo/cargo-release-plan/) explains
adoption; it is not required to execute the steps below. Repository-specific action pairing and
communication rules belong to the caller's instructions, not a sibling skill dependency.

The supported tool interface is `cargo-release-plan` 0.5.0, with version-plan/report schema 4
and semantic-decision schema 1. Verify the executable before modifying source. Do not assume
an arbitrary newer version preserves the copied skill's interface; update the skill together
with the selected tool or use the supported version. A repository developing the tool may
explicitly select its tested source-built executable instead of an installed release.

Required prerequisites are Git, the selected Cargo/Rust toolchain, `cargo-release-plan` and
`cargo-semver-checks`. GitHub CLI authentication is needed when the configured release
repository cannot be fetched anonymously. Do not install or upgrade tools without the caller's
applicable permission. Run from the selected Cargo workspace with the committed
`.cargo/release_plan.toml` configuration, or its explicit configured override.

# Placeholders and working files

| Placeholder | Meaning |
| --- | --- |
| `TOOL` | Installed `cargo-release-plan` executable, or an explicitly selected source-built executable. |
| `MANIFEST` | Absolute path to the selected workspace's `Cargo.toml`. |
| `CONFIG` | Publication configuration path relative to that workspace. Default: `.cargo/release_plan.toml`. |
| `WORK_DIR` | New absolute untracked evidence directory for this assessment. |
| `VERIFY_DIR` | Separate new absolute untracked directory for post-application evidence. |
| `BASE` | Frozen `release_base` commit from `context.json`, not a PR target branch. |
| `PACKAGE`, `DIFF_PATH` | Package name and its report-relative `diff_path`. |

The command examples use PowerShell; executable arguments have the same meaning on other
supported systems. Stop on any command failure unless a step explicitly defines an advisory
result. Retain diagnostics and valid evidence; do not turn missing or malformed output into
an empty successful plan.

Keep `context.json`, `prepared.json`, `report.json`, `diffs/`, `analysis-order.json`,
`decisions.json`, `plan.json`, `compatibility/`, and `preview/` under `WORK_DIR`. Captured files
are tool-owned: do not edit them by hand. Only `decisions.json` is authored by the agent.
Keep evidence, logs and temporary previews out of commits.

Every `check-compatibility --output` must name a directory that does not exist yet;
the command creates it. Create only `WORK_DIR` by hand, not `VERIFY_DIR`. When
repeating a comparison, choose a fresh name such as `compatibility-2` and use that
path consistently in subsequent reads. Source/preparation restarts use a new
`WORK_DIR`, preserving the prior evidence instead of overwriting it.

# Stage 1: Verify the tool and resolve the release context

> & "{{TOOL}}" --version
>
> git symbolic-ref --quiet --short HEAD
>
> New-Item -ItemType Directory -Path "{{WORK_DIR}}"
>
> & "{{TOOL}}" release-context --manifest-path "{{MANIFEST}}" --config "{{CONFIG}}" --verbose > "{{WORK_DIR}}/context.json"

Require the supported executable identity and a named feature branch. Stop on detached HEAD
or when the branch equals `release_branch` in `context.json`. Read `repository`,
`release_branch` and `release_base`; use that immutable `release_base` as `BASE` throughout.
The tool fetches the configured repository's actual release branch, without assuming a local
remote nickname or that the release branch is named `main`.

An unreleased stacked-PR parent is not a release baseline. A merge queue instead uses its
explicit tested release-branch base; repository CI supplies that input, not this feature-branch
authoring workflow.

> & "{{TOOL}}" check-published --manifest-path "{{MANIFEST}}" --verbose

Workspace discovery is advisory: report every missing or indeterminate package and continue
to assessment. The resolved-plan gate in Stage 6 is not advisory. A Git anchor's absence does
not establish that a package has never been published.

# Stage 2: Prepare and collect bound evidence

> & "{{TOOL}}" prepare --manifest-path "{{MANIFEST}}" --base "{{BASE}}" --output "{{WORK_DIR}}" --verbose
>
> & "{{TOOL}}" check-compatibility --manifest-path "{{MANIFEST}}" --prepared "{{WORK_DIR}}/prepared.json" --output "{{WORK_DIR}}/compatibility" --verbose

Preparation performs the intended offline workspace resolution and may update `Cargo.lock`.
It does not request blanket third-party upgrades. Read-only classification never resolves
dependencies. Compatibility execution uses the prepared source identity, runs the
external checker only for selected consumer contracts, and checks that source and resolution
remain unchanged. A broken checker or stale input is not evidence of compatibility.

Read every publishable package in `report.json.packages`, not only changed-file patches.
Each entry includes its status, declared version, optional anchor, changed inputs, dependencies
and public-exposure flags. File changes have patches; inherited workspace values and locked
binary dependency changes appear only in `changed`. Assess all of them.

Track any untracked source that this contribution intends to publish, then restart preparation.
Account for other untracked entries as deliberately unreleased. Private `publish = false`
members appear in `non_publishable_packages` for alignment only: do not assign them semantic
levels or query their registry status. Group members span both arrays.

# Stage 3: Determine assessment order

> & "{{TOOL}}" analysis-order --report "{{WORK_DIR}}/report.json" --verbose > "{{WORK_DIR}}/analysis-order.json"

Read the ordered package batches. Every publishable package appears once, dependency-first.
A cyclic batch represents actual mutually dependent packages; assess it until decisions settle.
Version grouping alone does not create a semantic-assessment cycle.

# Stage 4: Choose semantic decisions

For each batch, use [determining-level.md](determining-level.md) to judge the complete
released change since each package's anchor. Include pending increments already on the branch;
an existing increment is not an assessment of the changes accumulated beneath it.

Read `compatibility/compatibility.json` and the associated `semver-checks.log`.
Require `completed: true`. A compared package's `required_level` is a semantic floor:
`breaking` or `nonbreaking`; `null` supplies no minimum. A package with `compared: false` has
no comparison, not proof of compatibility. The checker does not cover all behavioral,
CLI, persisted-format or feature-gating promises; semantic judgment still belongs here.

Write decisions using semantic levels, not numeric increment levels:

```json
{
  "schema_version": 1,
  "changes": [
    { "name": "example-api", "level": "nonbreaking" },
    { "name": "example-tool", "level": "patch" }
  ]
}
```

Omit packages needing no semantic increment. Do not lower a decision below a checker floor.
Keep substantive reasons with the evidence, including inherited-only changes, locked binary
closures, public-dependency breaks and dependent requirement rewrites.

# Stage 5: Resolve and review the complete effects

> & "{{TOOL}}" propose --report "{{WORK_DIR}}/report.json" --decisions "{{WORK_DIR}}/decisions.json" --out "{{WORK_DIR}}/plan.json" --verbose
>
> & "{{TOOL}}" preview --manifest-path "{{MANIFEST}}" --prepared "{{WORK_DIR}}/prepared.json" --plan "{{WORK_DIR}}/plan.json" --output "{{WORK_DIR}}/preview" --verbose
>
> & "{{TOOL}}" check-compatibility --manifest-path "{{MANIFEST}}" --plan "{{WORK_DIR}}/preview/plan.json" --output "{{WORK_DIR}}/preview-compatibility" --verbose

The proposed plan is not the complete release set. Preview expands groups, requirement
rewrites and actual binary lockfile effects to a fixed point, retaining sufficient pending
increments rather than increasing them again. The resolved `preview/plan.json` names every
version target and captures the exact manifest/lockfile writes. Structural `expand` output
alone is not sufficient for this workflow.

Assess `preview/report.json`, its patches and final compatibility evidence. New dependency
effects establish a minimum, not semantic compatibility. Raise `decisions.json` where needed,
then repeat proposal/preview against the original prepared report, using new compatibility
output directories. Never synthesize edits to generated plans or widen exact requirements
to remove unwanted group members. Source edits require new preparation.

Prepare the PR's **Version/release plan** section from the union of resolved-plan targets and
all pending-release entries in the final report. Use one row per complete version group and
one per ungrouped package. Include every member, previous anchor version, proposed version,
semantic level and substantive reason. Use current helper versions only as alignment starting
points, not invented published predecessors. Mark nonpublishable members **version alignment
only, not published**. Explain retained increments, group-only alignment, public dependency
effects and prospective dependency changes. State explicitly when nothing is to release.

First-publication packages get a separate handoff: name the package, lack of Git anchor,
known registry state, bootstrap version and higher intended first automated-release version.
A maintainer bootstraps in dependency order from the feature branch before its first merge and configures Trusted
Publishing; the first merge performs the second publication. Do not publish from this skill.

# Stage 6: Refresh context and apply unchanged

> & "{{TOOL}}" release-context --manifest-path "{{MANIFEST}}" --config "{{CONFIG}}" --verbose > "{{WORK_DIR}}/refreshed-context.json"
>
> & "{{TOOL}}" check-published --manifest-path "{{MANIFEST}}" --plan "{{WORK_DIR}}/preview/plan.json" --verbose

Compare the refreshed release baseline with `BASE`. If it moved, recover as described below.
The plan-scoped registry check must succeed for every publishable resolved target; private
alignment members are excluded. Stop for first-publication or unknown-state blockers.

Preserve the exact pre-application file state and the versioning-only delta this run produces.
Then apply without a separate approval prompt:

> & "{{TOOL}}" apply --manifest-path "{{MANIFEST}}" --plan "{{WORK_DIR}}/preview/plan.json" --verbose

Application must use the captured resolved artifact unchanged. It does not perform a late
resolver refresh. Stale inputs require fresh preparation; partial I/O failure requires inspecting
and reporting the affected files before any further planning.

# Stage 7: Verify and hand off

> cargo metadata --manifest-path "{{MANIFEST}}" --locked --format-version 1
>
> & "{{TOOL}}" check-compatibility --manifest-path "{{MANIFEST}}" --base "{{BASE}}" --output "{{VERIFY_DIR}}" --verbose
>
> & "{{TOOL}}" check --manifest-path "{{MANIFEST}}" --base "{{BASE}}" --config "{{CONFIG}}" --format github --verbose

The fresh compatibility operation writes a read-only report and bound comparison evidence.
Check its `completed` state and floors, not only the process exit. Verify every original
decision and resolved target against that report, preserving original preparation evidence.
The lockfile check must not repair anything. Refresh independently maintained fixture lockfiles
only under the repository's rules and reassess any resulting released-content effects.

Commit the intended source/version/requirement/lockfile changes and keep the PR section current.
Follow the repository's communication rules and post-version integration instructions.
Summarize substantive decisions, incomplete comparisons, first-publication handoffs and any
execution blockers. If posting diagnostics on GitHub, keep them in a collapsible section rather
than embedding local paths or a validation transcript in the PR's release table.

# Recovery

Release-branch movement is expected. Undo only the verified generated versioning delta from
this run, preserving independently authored changes; merge the refreshed release baseline into
the feature branch using the repository's normal workflow, then restart with new evidence
directories. Do not reset the worktree, overwrite mixed manifest edits or increment stale
proposed versions manually. Stop for ambiguous partial application or unresolved conflicts.

Changed source, group membership or configuration requires preparation again. Changed semantic
decisions alone require proposal and preview again. Manifest defects are not semantic choices:
fix malformed/stale requirements or invalid configuration directly, then regenerate evidence.
Completing this workflow remains neither merge approval nor publication authority.
