# Scope

Prepare the version changes for one pull request in a repository that uses the
cargo-release-plan toolkit for publication. Confirm that adoption from the
repository's instructions or publishing workflow before proceeding; a globally
installed skill must not operate on an unrelated repository.

The skill assesses released changes, chooses semantic decisions, previews their
dependency effects and applies the resulting version plan. Review of the complete
pull request approves the result; do not introduce a separate approval pause for
version choices. This skill does not authorize merging, publication, tag creation
or Trusted Publisher administration.

The CLI, this skill and the reusable GitHub workflows form one toolkit. The
[public guide](https://folo-rs.github.io/folo/cargo-release-plan/) explains adoption.
Repository-specific action pairing and communication rules belong to the caller's
instructions.

# Prerequisites

Git, the selected Cargo/Rust toolchain, `cargo-release-plan`, and
`cargo-semver-checks` must be available. Do not install or upgrade tools without
the caller's applicable permission. Fetching a private configured repository also
requires GitHub CLI authentication.

Check the schema revisions reported by `cargo release-plan version` before
modifying source. This skill consumes plan/report/prepared schema `5`,
semantic-decision schema `1`, compatibility schema `1` and release-context schema
`2`. Matching package version numbers are not a prerequisite.

If schemas differ, update the installed skill from the complete
[canonical skill directory](https://github.com/folo-rs/folo/tree/main/.github/skills/increment-versions),
including its command reference, decision guide and license, and restart with its
instructions. If already working in the canonical repository, report the mismatch
to the caller instead of overwriting the skill under development. If unexpected
CLI errors suggest that tool and skill have diverged despite matching schemas,
consider upgrading both to their latest compatible revisions. Inspect any partial
changes before restarting; an upgrade is not permission to repeat a failed write blindly.
If the updated skill still does not support the tool's schemas, stop and report
the mismatch rather than repeatedly copying the same revision.

Run in the selected Cargo workspace with its working-tree
`.cargo/release_plan.toml`, or an explicit configuration override. Existing tracked
files may have staged or unstaged edits; a clean checkout or prior commit is not
required. Newly created release inputs must be tracked before assessment. Stage
those specific paths without disturbing unrelated changes, then collect fresh
evidence. Version planning reads the working tree, not just the staged diff.

# Assessment model

The **release history** is the actual release branch up to one selected commit.
A package's **anchor** is the newest first-parent commit in that history where
its parsed version changed. It supplies the package's comparison version and
content. Changes after an anchor remain relevant, allowing a repository to catch
up after migration or manual changes without matching version increments.

The **merge target** is the commit the current pull request intends to merge into.
For a stacked PR it can be the tip of an unmerged parent. Supply that target
separately from release history. A parent's pending version and final content are
assessed together as its anticipated squash release; its intermediate commits
do not define separate releases. Additional child changes need their own version
movement. Packages still at their release-history version retain their historical
anchors and catch-up obligations.

The managed workflow uses squash merges so each merged PR records its final
content and versions together. Assessment does not depend on whether a merge queue
groups PRs. After a parent merges, refresh the history and target rather than
continuing to use stale evidence.

A **version group** connects tracked workspace members through exact local
requirements such as `=1.2.3`; their versions align. A compatible requirement such
as `1.2.3` permits later compatible releases and does not create a version group.

A **semantic decision** is a `breaking`, `nonbreaking` or `patch` judgment produced
by this skill. A **version plan** translates those decisions into package versions
and required group/dependency changes.

A package has a **pending increment** when its working-tree version is above its
comparison version. Assess every change assigned to that increment. Keep it when
it is sufficient; raise it when a stronger decision requires more movement. Do
not erase or increment it again merely because the skill was rerun.

A **non-publishable version target** has `publish = false`. It can participate in
group alignment but receives no publication request or semantic decision. A
publishable private-API package is different: it still has released content.

# Evidence and commands

Read [commands.md](commands.md) for the command examples, placeholder table,
PowerShell error handling and expected outputs. Read the matching command section
before executing each numbered stage below.

Choose new absolute evidence directories outside the repository or under an
already ignored location. The tool writes reports, patches, plans and supporting
files there. Do not edit these generated files manually. The semantic
`decisions.json` is the agent-authored input. Keep all working evidence out of commits.

Every compatibility output directory must be new. When repeating a stage, choose
new destinations and retain earlier evidence. Record diagnostics as they occur;
missing or malformed output is never an empty successful plan.

# Stage 1: Check prerequisites and select history

Follow [initialization](commands.md#stage-1-check-prerequisites-and-select-history).
Require matching schemas and a named non-release branch. For a stacked PR, obtain
its actual target branch/ref from the PR or established session context; do not
silently substitute the repository's release branch.

Read the repository, release branch, `release_history` and optional `merge_target`
from the generated context. Use those exact commits throughout this assessment.
Keep the supplied target ref as well so refresh can detect parent movement.

Run advisory workspace registry discovery. Record every missing or indeterminate
package and continue assessment; the later plan-scoped gate is mandatory.

# Stage 2: Prepare complete evidence

Follow [preparation](commands.md#stage-2-prepare-complete-evidence).
The outcome is a consistent workspace lockfile and a report bound to the source
being assessed. Preparation may update `Cargo.lock`; inspect and retain that
change as part of the contribution.

Read every publishable package in `report.json.packages`, not only file patches.
Include inherited manifest changes and binary dependency changes listed in
`changed`. Read `diff_path` relative to the report directory when present.
Record excluded untracked package paths and their rationale; track intended
release inputs and restart instead of silently omitting them.

Non-publishable targets appear separately for alignment. Do not assign them
semantic decisions or query their registry publication status.

# Stage 3: Assess in dependency order

Follow [assessment ordering](commands.md#stage-3-assess-in-dependency-order).
Read the ordered package batches. Each publishable package appears once,
dependency-first. Assess a cyclic batch until its mutually dependent decisions
settle. Version grouping alone does not create a semantic-assessment cycle.

# Stage 4: Choose semantic decisions

Use [determining-level.md](determining-level.md) for the complete released change
in each package, including all changes covered by a pending increment.

Require `completed: true` in compatibility evidence. A compared package's
`required_level` supplies a minimum semantic decision; a null value imposes no
minimum, and `compared: false` is not proof of compatibility. The skill's wider
judgment of behavior, CLI, data formats and feature contracts can raise that floor.

Write `decisions.json` with semantic levels, not numeric increments. Omit packages
requiring no semantic increment. Record substantive reasons, including inherited
changes, dependency effects and retained pending versions. Do not invent a Git
anchor for a package absent from release history and the anticipated parent.

# Stage 5: Review resolved effects

Follow [proposal and preview](commands.md#stage-5-review-resolved-effects).
Preview produces the complete version target set and exact manifest/lockfile
edits, including effects discovered through dependency resolution.

Read its report, patches and compatibility results. Assess new effects and raise
decisions where needed, then repeat proposal/preview from the original prepared
evidence with new output directories. Source edits require new preparation.
Never hand-edit generated plans or widen exact requirements to evade grouping.

Prepare the PR's **Version/release plan** from the union of resolved-plan targets
and pending releases in the final report. Give one row per complete group or
ungrouped package: previous comparison version, proposed version, semantic level
and substantive reason. Explain dependent/group movements and retained versions.
Mark non-publishable targets **version alignment only, not published** without
inventing a published predecessor. State explicitly when nothing is to release.

## First-publication handoff

For packages with no comparison anchor, record the known registry state, bootstrap
version and higher first automated-release version. A maintainer bootstraps the
registry identities before first merge and configures Trusted Publishing.
Bootstrap occupies its version, so the automated release needs a higher one.
Do not publish from this skill or infer registry absence from Git history.

# Stage 6: Refresh and apply

Follow [application](commands.md#stage-6-refresh-and-apply).
Refresh the selected release history and target ref. If either changed, use the
recovery procedure below. Require the plan-scoped registry gate to pass for every
publishable target.

Preserve the pre-application file state and identify the versioning-only delta.
Apply the resolved artifact unchanged, without another approval prompt. Inspect
and report any partial I/O failure before further planning.

# Stage 7: Verify and hand off

Follow [verification](commands.md#stage-7-verify-and-hand-off).
Require complete fresh compatibility evidence and a passing configured check.
Verify every decision and resolved target against the fresh report. Preserve the
original assessment and account for all intended tracked release inputs.

Commit the intended source/version/requirement/lockfile changes and keep the PR's
release plan current. Follow repository-specific post-version instructions.
Summarize substantive decisions, incomplete comparisons, first-publication work,
excluded untracked paths and execution blockers. When posting on GitHub, put
execution diagnostics in a collapsible section rather than the release table.

# Recovery

When release history or the target advances, undo only the verified generated
versioning delta from this run. Preserve independently authored changes, update
the branch to the intended target and restart with new evidence directories.
Do not reset the worktree or overwrite mixed manifest edits.

Changed source, groups or configuration also require fresh preparation. Changed
semantic decisions alone require proposal and preview again. Fix manifest defects
directly, then regenerate evidence; a malformed requirement is not a semantic choice.
