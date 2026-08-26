---
name: increment-versions
description: Propose and apply crate version increments for a pull request that changed released content. Use when the validate-versions check fails, when a pull request is ready to merge, or when the user asks to bump crate versions.
---

# Scope

A package is released by incrementing its version. A pull request that changes a package's
released content must also increment that package's version. This skill proposes one
increment *level* per version group and per ungrouped package. The author may raise a
level above the `cargo-semver-checks` floor; they may not lower one.

Judgement lives here. Mechanics live in `just` recipes. The only question this skill asks
is the increment *level*. Group expansion, `=`-pin rewrites, the lockfile refresh, and
`just verify-lockfile` have no skip answer: skipping a group member diverges the group,
skipping a pin leaves a stale `=` requirement, and skipping the lockfile fails `--locked`
builds.

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

Gather the classification report and the `cargo-semver-checks` floor.

> just release-report "{{OUT_DIR}}"

Placeholders to replace:

| Placeholder | Description |
|-------------|-------------|
| `OUT_DIR` | A working-tree directory that is **not** committed (for example a path under the runner temp directory, or `.release-plan/` which must stay untracked). Receives `report.json`, `diffs/<package>.patch`, and `semver-checks.log`. |

If `just release-report` fails before writing `report.json`, stop and report the error.
`cargo-semver-checks` findings are captured in `semver-checks.log` even when that tool
exits non-zero; that non-zero exit is the floor, not a broken tool (the canary already
ran). Continue to propose.

Optional: set `RELEASE_PLAN_BASE` to the pull request base SHA when not comparing against
`origin/main`. Leave it unset for a normal local run.

Read `{{OUT_DIR}}/report.json` for per-package status, diffs, groups, dependencies, and
dependents. Cite diffs by path (`{{OUT_DIR}}/diffs/<package>.patch`); do not paste them.

# Stage 3: Propose

If a never-published crate from Stage 1 would enter the increment set, **stop**. Do not
fold it into `apply`.

Walk the workspace dependency graph in topological order (dependents after dependencies).
Per package that is `unreleased-changes` or that a decided increment will drag in:

1. Take the `cargo-semver-checks` floor from `semver-checks.log`. If the log does not name
   a required bump for that package, the floor is none (a patch is still allowed).
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

On approval, write the plan JSON to a working-tree path that will not be committed, then:

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

Then re-run `cargo-semver-checks` scoped to the packages whose versions just moved (the
same set `validate-versions` emits as `released` in CI):

> just package="{{PACKAGES}}" semver-checks

Placeholders to replace:

| Placeholder | Description |
|-------------|-------------|
| `PACKAGES` | Space-separated package names that the plan incremented and that have a consumer-visible API (omit `*_impl` crates). Empty means skip. |

If the command reports any errors, stop and report them. If it completes successfully,
continue.

Further changes may follow the increment without invalidating it.

Do not commit the plan file. Do commit the manifest, pin, and `Cargo.lock` edits.

Write a short summary of the decided levels (and where they exceeded the floor) into the
pull request description. Prefix any GitHub comment or description edit with
`[Copilot speaking]`.

# Diagnostics

Include in the summary: which packages or groups were considered, the
`cargo-semver-checks` floor versus the chosen level for each row, any group expansion or
`=`-pin propagation, and whether propose stopped on a never-published crate. If the
summary is posted as a GitHub comment, put those diagnostics in a collapsible section.
