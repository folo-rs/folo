---
name: increment-versions
description: Propose and apply crate version increments for a pull request that changed released content. Use when the validate-versions check fails, when a pull request is ready to merge, or when the user asks to bump crate versions.
---

# Scope

An **increment** raises a package's declared version so **unreleased changes**
become **pending release**. Publishing those versions is the separate release
process. This skill only chooses and applies increment *levels*.

This skill is the recovery procedure named by the `validate-versions` check. It
proposes one increment *level* per **version group** and per ungrouped package.
The author may raise a level above the `cargo-semver-checks` floor; they may not
lower one.

`docs/git-workflow.md` still forbids committing crate version bumps on feature
branches. Do not run Stage 5 unless the caller explicitly asked to increment
versions on this branch.

Judgement lives here. Mechanics live in `just` recipes. The only question this
skill asks is the increment *level*. Group expansion, `=`-pin rewrites, the
lockfile refresh, and `just verify-lockfile` have no skip answer: skipping a
group member diverges the group, skipping a pin leaves a stale `=` requirement,
and skipping the lockfile fails `--locked` builds.

Do not commit the plan file. The version check verifies manifest state, not intent.

# Stage 1: Preflight

Prove that `cargo-semver-checks` can run, and collect the never-published crate list that
Stage 3 treats as a stop. A `cargo-semver-checks` that fails to *run* (classically one too
old for the toolchain's rustdoc JSON format) must never be read as "no breaking changes".
A never-published crate cannot be first-published by crates.io Trusted Publishing, so it is
not folded into `apply`.

Run the canary:

> just verify-semver-checks

Placeholders to replace: none.

If the command reports any errors, stop and report the error to the caller. Do not continue
to collect. If it completes successfully, continue.

Then list never-published crates:

> just check-never-published

Placeholders to replace: none.

The recipe warns (it is not a gate). Keep the list of never-published crates and apply it
in Stage 3: a never-published crate in the increment set is a **stop**, not a bump.
First-publish is a manual `cargo publish` after merge, documented in `RELEASING.md`. If
the recipe cannot confirm status, stop and ask the caller to verify rather than guessing.

# Stage 2: Collect

Gather the classification report and the `cargo-semver-checks` floor. That evidence
describes the released-content snapshot it analyzed.

> just release-report "{{OUT_DIR}}"

Placeholders to replace:

| Placeholder | Description |
|-------------|-------------|
| `OUT_DIR` | A working-tree directory that is **not** committed (for example a path under the runner temp directory, or `.release-plan/` which must stay untracked). Receives `report.json`, `diffs/<package>.patch`, and `semver-checks.log`. |

If `just release-report` fails before writing `report.json`, stop and report the error.
`cargo-semver-checks` uses exit 0 for a clean comparison and 100 when it found an
increment floor; that non-zero exit is the floor, not a broken tool (the canary already
ran). Any other exit is a tool failure: stop. Continue to propose only after a 0 or 100.

Optional: set `RELEASE_PLAN_BASE` to the pull request's release baseline SHA when not
comparing against the tool default. Leave it unset for a normal local run.

Read `{{OUT_DIR}}/report.json` for per-package status, diffs, groups, dependencies, and
dependents. Cite diffs by path (`{{OUT_DIR}}/diffs/<package>.patch`); do not paste them.

# Stage 3: Propose

If a never-published crate from Stage 1 would enter the increment set, **stop**. Do not
fold it into `apply`.

Walk the workspace dependency graph in topological order (dependents after dependencies).
Per package whose status is `needs-increment`, or that a decided increment will drag in:

1. Take the `cargo-semver-checks` floor from `semver-checks.log`. If the log does not name
   a required bump for that package, the floor is none (a patch is still allowed). For a
   public shell, the floor includes changes re-exported from a grouped `_impl` member.
2. Read the package's diff and decide a level. Raise above the floor when the change is an
   undetectable behavioural break, a meaningful feature addition, or needed to keep a
   version group aligned. Never lower below the floor.
3. Every crate here is `0.x` or `1.x`. Under Cargo's semantics a breaking change to a `0.x`
   crate is a **minor** increment, not major.
4. Expand version groups: if any member needs an increment, all members take the highest
   level any member requires, applied to the highest version any member declares.
5. Propagate `=` pins: incrementing an `=`-pinned dependency is itself a released-content
   change in the dependent and requires its own increment. Re-check that expansion did not
   create new work.

A `pending-release` package is already past its version-anchor. Raise it further only when
the `cargo-semver-checks` floor exceeds the already-applied increment.

The plan schema (uncommitted) is:

```json
{
  "schema_version": 1,
  "increments": [
    { "name": "nm", "level": "patch" },
    { "name": "events", "version": "0.7.14" }
  ]
}
```

`name` is a package name or a version-group name. Each increment supplies exactly one of
`level` (`major`, `minor`, `patch`) or `version`.

# Stage 4: Present

Show **one row per version group and per ungrouped package**, not one row per crate. A
version group is one decision.

Each row: current version, proposed version, level, the `cargo-semver-checks` floor, the
members the level will apply to, and a one-line justification citing the actual change.
Where the proposal exceeds the floor, state the reason explicitly.

Ask the caller to approve or adjust levels. Do not apply until approved.

# Stage 5: Apply

On approval, and only when the caller explicitly asked to increment versions on this
branch, write the plan JSON to a working-tree path that will not be committed, then:

> just apply-release-plan "{{PLAN}}"

Placeholders to replace:

| Placeholder | Description |
|-------------|-------------|
| `PLAN` | Path to the approved plan JSON from Stage 3. |

If apply reports any errors, stop and report them. If it completes, refresh-check the
lockfile:

> just verify-lockfile

Placeholders to replace: none.

If `verify-lockfile` fails, stop; apply already refreshed the lockfile, so a failure here
means the tree is inconsistent and must not be pushed.

Then confirm the manifests:

> just validate-versions

Placeholders to replace: none.

If `just validate-versions` reports any errors, the plan missed a package; return to Stage 3.
If it completes successfully, continue.

Then re-run `cargo-semver-checks` scoped to the consumer-visible packages whose versions
just moved (the same set `validate-versions` emits as `released` in CI):

> just package="{{PACKAGES}}" semver-checks

Placeholders to replace:

| Placeholder | Description |
|-------------|-------------|
| `PACKAGES` | Space-separated package names that the plan incremented and that have a consumer-visible API (omit `*_impl` crates; include the public shell when a grouped `_impl` member changed). Empty means skip. |

If the command reports any errors, stop and report them. If it completes successfully,
continue.

If later edits change published content, return to Stage 2 and re-decide before treating
the increment as final. Edits outside published artifacts do not invalidate the snapshot.

Do not commit the plan file. Do commit the manifest, pin, and `Cargo.lock` edits.

Write a short summary of the decided levels (and where they exceeded the floor) into the
pull request description. Prefix any GitHub comment or description edit with
`[Copilot speaking]`. Keep candidate lists, floors, and stop diagnostics in the collect
directory or a snapshot-labelled comment, not in the pull request description.

# Diagnostics

Retain with the snapshot: which packages or groups were considered, the
`cargo-semver-checks` floor versus the chosen level for each row, any group expansion or
`=`-pin propagation, and whether propose stopped on a never-published crate. If that
evidence is posted as a GitHub comment, put it in a collapsible section labelled as
belonging to that collect run.
