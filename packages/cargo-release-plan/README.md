# cargo-release-plan

A Cargo subcommand that classifies every publishable workspace package against its
version **anchor**, reports changes to **released content**, and applies an
approved increment plan (group expansion, `=`-pin rewrites, lockfile refresh).

A package has unreleased changes when its released content differs between its
version anchor and the work tree without an increase in its declared version.
The anchor is the most recent commit on the base revision's first-parent line in
which the package's parsed `version` changed.

## Usage

Install with [`cargo binstall cargo-release-plan`](https://github.com/cargo-bins/cargo-binstall)
to fetch a prebuilt binary on supported targets (transparently building from source
elsewhere), or `cargo install cargo-release-plan` to always build from source. Then:

```text
cargo release-plan report --out-dir <dir> [--base <rev>] [--manifest-path <path>] [--verbose]
cargo release-plan check [--base <rev>] [--manifest-path <path>] [--format text|github] [--verify-packaging] [--verbose]
cargo release-plan apply --plan <plan.json> [--dry-run] [--manifest-path <path>] [--verbose]
```

Without `--base`, the base revision comes from the workspace's
`[workspace.metadata.release-plan] base` key, falling back to `origin/main`. CI
should pass an explicit SHA of the merge-base or target-branch tip (the **base
revision**). `--manifest-path` defaults to `Cargo.toml` in the current
directory.

### `report`

Writes `<dir>/report.json` plus a `<dir>/diffs/<package>.patch` for each package
whose unreleased changes include a file difference. The JSON names each
package's status, anchor, changed paths, inherited workspace fields,
intra-workspace dependencies, and version groups, and is the complete verdict.

Each `.patch` is a zero-context unified diff in the shape `diff -U0` produces,
so it can be piped into standard tooling. Inherited workspace value changes are
not diffs and appear only as `changed` entries with `source: "inherited"`, so a
package that fails on an inherited value alone has no patch. Enumerate `status`
in `report.json` rather than the `diffs/` directory to find every package that
needs an increment.

### `check`

Exits non-zero when any publishable package has unreleased changes or any
version group declares inconsistent versions. Failure text describes the
self-contained recovery workflow: run `report`, prepare a plan, and run `apply`.
In this repository it additionally names the `increment-versions` agent skill,
which automates that workflow.

`--format github` also emits GitHub Actions workflow annotations.

`--verify-packaging` cross-checks this tool's released-content rules against
`cargo package --list`. Divergences are printed as warnings and do not fail the
check: listing a package makes Cargo require a clean work tree, resolve the
dependency graph, and assemble the package archive, so gating on it would trade
the tool's offline, no-resolve guarantee for false failures. A divergence is
evidence that the rules need fixing, not a condition to tolerate.

### `apply`

Reads an approved plan and:

* sets each listed package's `version`
* expands version groups so every member receives the new version
* rewrites intra-workspace dependency requirements that must follow, including
  `=` pins
* refreshes the workspace lockfile so `--locked` builds keep working

Manifests are edited structurally, so comments and layout are preserved. Every
reason a plan can be rejected is found before anything is written: an unknown
target, a version that would move backwards, or an unreadable manifest is
reported while every manifest is still untouched. Writes themselves are
sequential, so an accepted plan that then fails on an I/O error can leave earlier
manifests updated; revert the work tree with `git` in that case. `--dry-run`
reports the manifests that would change and writes nothing.

The plan schema is:

```json
{
  "schema_version": 1,
  "increments": [
    { "name": "nm", "level": "patch" },
    { "name": "events", "version": "0.7.14" }
  ]
}
```

`name` is a package name or a version-group name. `level` is `major`, `minor`,
or `patch`. An explicit `version` is used as-is for that target (and its group),
and is rejected when it is lower than a version the target already declares.
Each increment must supply exactly one of `level` or `version`.

### Plan and report schema

`report.json` uses the same schema revision. Top-level fields are
`schema_version`, `head`, `packages`, and `groups`. Each package object includes
`name`, `declared_version`, `status` (`releasing` / `unreleased-changes` /
`released`), `changed`, `stat`, `dependencies`, and `dependents`, plus omitted
when empty: `group`, `anchor`, `diff_path`, `untracked`. A change is either
`{"path","change","source":"package"}` or `{"field","source":"inherited"}`.
`diff_path` is relative to the report directory. Plan and report formats
advance this revision together: an incompatible field, enum, or path-layout
change increments it.

## Classification

| Status               | Condition                                                | Verdict  |
| -------------------- | -------------------------------------------------------- | -------- |
| `releasing`          | version increased since the anchor                       | pass     |
| `unreleased-changes` | version unchanged, released content changed since anchor | fail     |
| `released`           | version unchanged, released content unchanged            | pass     |

Packages with `publish = false` are ignored. A package's own `Cargo.lock` is
never released content. Untracked files are advisory only. Versions only move
forwards: a
declared version below the anchor's version is an error rather than a status.

A packaged file's executable bit is released content too, since Cargo carries
the mode Git records into the archive. Making a packaged file executable is
therefore a change even when its bytes are untouched. The mode is read from the
index, so a checkout on a platform without executable permissions classifies the
same way.

Version groups are declared in the workspace root as
`[workspace.metadata.release-plan.groups]`. Members share a declared version; if
any member needs an increment, all members increment. Members that do not exist
on the base revision are exempt from the consistency rule.

## Offline operation

Classification shells out only to `git` and `cargo metadata --no-deps`. It does
not contact crates.io, resolve a dependency graph, or compile. `check
--verify-packaging` may spawn `cargo package --list`. `apply` may spawn
`cargo update --offline` to refresh the workspace lockfile.
