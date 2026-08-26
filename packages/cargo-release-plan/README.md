# cargo-release-plan

A Cargo subcommand that classifies every publishable workspace package against its
version **anchor**, reports unreleased content, and applies an approved increment
plan (group expansion, `=`-pin rewrites, lockfile refresh).

The version increment is the release event. A package fails the check when its
released content differs from the last parsed `version` change on the base
branch's first-parent line, and the declared version has not increased.

## Usage

Install with [`cargo binstall cargo-release-plan`](https://github.com/cargo-bins/cargo-binstall)
to fetch a prebuilt binary on supported targets (transparently building from source
elsewhere), or `cargo install cargo-release-plan` to always build from source. Then:

```text
cargo release-plan report --out-dir <dir> [--base <rev>] [--manifest-path <path>] [--verbose]
cargo release-plan check [--base <rev>] [--manifest-path <path>] [--format text|github] [--verify-packaging] [--verbose]
cargo release-plan apply --plan <plan.json> [--dry-run] [--manifest-path <path>] [--verbose]
```

`--base` defaults to `origin/main`. `--manifest-path` defaults to `Cargo.toml` in
the current directory.

### `report`

Writes `<dir>/report.json` plus one `<dir>/diffs/<package>.patch` per package
with unreleased changes. The JSON names each package's status, anchor, changed
paths, inherited workspace fields, intra-workspace dependencies, and version
groups.

### `check`

Exits non-zero when any publishable package has unreleased changes or any
version group declares inconsistent versions. Failure text names the
`increment-versions` skill.

`--format github` also emits GitHub Actions workflow annotations.

`--verify-packaging` cross-checks this tool's released-content rules against
`cargo package --list`. Divergences are printed as warnings and do not fail the
check.

### `apply`

Reads an approved plan and:

* sets each listed package's `version`
* expands version groups so every member receives the new version
* rewrites intra-workspace dependency requirements that must follow, including
  `=` pins
* refreshes the workspace lockfile so `--locked` builds keep working

Manifests are edited with `toml_edit` (comments and layout preserved). Every
edit is computed before any file is written.

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
or `patch`. An explicit `version` is used as-is for that target (and its group).
Each increment must supply exactly one of `level` or `version`.

## Classification

| Status               | Condition                                                | Verdict  |
| -------------------- | -------------------------------------------------------- | -------- |
| `releasing`          | version increased since the anchor                       | pass     |
| `unreleased-changes` | version unchanged, released content changed since anchor | fail     |
| `released`           | version unchanged, nothing released-relevant changed     | pass     |

Packages with `publish = false` are ignored. `Cargo.lock` is never released
content. Untracked files are advisory only.

Version groups are declared in the workspace root as
`[workspace.metadata.release-plan.groups]`. Members share a declared version; if
any member needs an increment, all members increment. Members that do not exist
on the base revision are exempt from the consistency rule.

## Offline operation

The tool shells out only to `git` and `cargo metadata --no-deps`. It does not
contact crates.io, resolve a dependency graph, or compile as part of
classification.
