# Artifact reference

Local planning and post-merge publication serve different purposes and have
separate schema lifecycles. Use tool-produced evidence where specified; do not
fabricate captured inputs, identities or outcome receipts.

## Local decisions and plans

A decisions document is caller-authored literal JSON:

```json
{
  "schema_version": 1,
  "changes": [
    { "name": "widget", "level": "nonbreaking" },
    { "name": "widget_impl", "level": "patch" }
  ]
}
```

`changes[].level` is semantic: `breaking`, `nonbreaking` or `patch`.
Nonpublishable targets have no semantic decisions.

The proposed-plan format uses report/plan schema revision `4`. This is a literal
example of that format:

```json
{
  "schema_version": 4,
  "increments": [
    { "name": "widget", "level": "minor" },
    { "name": "widget-cli", "version": "2.0.1" }
  ]
}
```

`increments[].level` is numeric: `major`, `minor` or `patch`. Supply exactly one
of `level` and `version` per entry. Names select tracked workspace members;
group members expand together. Explicit targets cannot lower declared versions,
and group targets use plain `major.minor.patch` versions.

An expanded plan has `expanded` set, names every version target and supplies
explicit versions. Structural expansion alone is not sufficient for complete
release application. `preview` adds captured `resolved` evidence, including
resolved file contents, original inputs and `evidence_manifest_path`.

Treat `prepared.json` and the `resolved` object as opaque. Preserve them intact;
regenerate unsupported or stale evidence with `prepare` and `preview`.

## Reports

The revision-4 report's top level contains:

| Field | Content |
| --- | --- |
| `schema_version` | Report/plan format revision. |
| `head` | Source commit associated with the assessment. |
| `packages` | Publishable package assessments. |
| `non_publishable_packages` | Tracked alignment targets without release assessments. |
| `groups` | Complete group membership across both package arrays. |

Each publishable entry includes `name`, `declared_version`, `status`, `changed`,
`stat`, `dependencies`, `dependents` and `consumer_contract`. Optional evidence
includes its group, anchor, patch path and advisory untracked files.

Changed entries distinguish:

| `source` | Evidence |
| --- | --- |
| `package` | Packaged file path and change kind. |
| `inherited` | Changed inherited workspace field. |
| `lockfile` | Changed binary installation dependency identity. |

Dependencies record `name`, `req`, `exact_pin` and `public`. Consumer-contract
flags select public library comparisons; they do not claim that binaries or
private implementation changes lack behavioral consequences.

Nonpublishable entries carry `name`, `declared_version` and an optional group,
not a status or semantic change level. Group records include complete sorted
members and their highest declared version.

Patch paths are relative to the report directory. Patches are zero-context
unified file diffs, not the complete verdict. An inherited-only or lockfile-only
change can have no patch.

A report's HEAD alone does not prove that an arbitrary dirty checkout matches
the evidence. `check-compatibility` regenerates a bound read-only report from
prepared inputs, a resolved preview or a fresh source assessment. It does not
accept a detached report. This leaves the report/plan schema unchanged.

## Release context

`release-context` prints JSON with these fields:

| Field | Meaning |
| --- | --- |
| `repository` | Configured GitHub `owner/repository`. |
| `release_branch` | Configured release branch. |
| `release_base` | Resolved immutable history boundary for this invocation. |
| `head` | Assessed source HEAD. |
| `workspace_manifest` | Repository-relative workspace manifest location. |
| `config_path` | Repository-relative configuration location. |
| `concurrency_group` | Stable workspace-scoped release concurrency identity. |

This is acquired context, not publication intent. Save it for the local planning
run and reuse its `release_base`. Refresh before application; do not silently
replace the baseline beneath prepared evidence.

## Compatibility evidence

Each `check-compatibility` invocation uses a new output directory and writes
`compatibility.json`, `semver-checks.log` and a regenerated read-only report.
The evidence retains checker identity and exact published comparison versions.

Require `completed: true` before using the result. A package's `compared: false`
means no comparison was available, not proof of compatibility.
`required_level` is a semantic `breaking` or `nonbreaking` floor; `null`
establishes no minimum. The author still judges behavioral, CLI, format and
feature-subset effects.

Do not confuse a completed comparison with a passing merge gate:
`--deny-findings` additionally rejects insufficient increments. Captured source
is verified around comparison; operational errors never become semantic passes.

## Artifact-only command output

`analysis-order` emits a dependency-first array:

```json
[
  { "order": 1, "packages": ["widget_impl"], "cyclic": false },
  { "order": 2, "packages": ["widget"], "cyclic": false },
  { "order": 3, "packages": ["widget-cli"], "cyclic": false }
]
```

This example assumes those are the publishable packages. Every publishable
member appears once, including unchanged ones; nonpublishable helpers do not.
Cycle batches represent actual dependencies, not group alignment.

`semver-targets` emits sorted package names, or `[]` when no public contract is
selected. `inspect-plan` emits:

```json
{
  "publication_targets": ["widget", "widget-cli", "widget_impl"],
  "evidence_manifest_path": null
}
```

This example represents structural inspection without retained preview evidence.
For a resolved artifact, the manifest path identifies the verified prospective
workspace. Request `--require-resolved` before relying on that workspace for the
complete local release flow.

## Publication manifest

`prepare-publish` creates this envelope:

```text
{
  id,
  publication: {
    schema_version: 1,
    tool_version,
    source,
    workspace_manifest,
    config_path,
    configuration,
    packages: [
      { name, version, manifest, binary: null | { name, targets } }
    ]
  }
}
```

This is a **field-shape sketch, not JSON input**. `source` is the full publication
commit. Workspace, package and configuration paths are repository-relative.
`configuration` captures the effective committed configuration. A binary's
`name` is the executable name, not necessarily its package name.

The package array includes every publishable exact version at that source,
including unchanged packages, and excludes nonpublishable alignment targets.
The artifact records producer identity and schema independently of local-plan
schemas.

`id` links subsequent work to the captured payload. It is an integrity and
linkage check, not a signature or independent authorization. A fetched branch
tip is not part of the captured intent digest. Do not change the payload,
recompute its identity by hand or add mutable publication flags.

## Registry outcome

Registry outcomes use schema `1` and record:

| Field | Meaning |
| --- | --- |
| `schema_version` | Outcome format revision. |
| `publication_id` | Original publication manifest identity. |
| `phase` | `registry`. |
| `dry_run` | Whether the attempt only observed intended work. |
| `complete` | Whether this live registry attempt established completion. |
| `packages` | Entries with exact `name`, `version` and `state`. |
| `errors`, `notes` | Failure diagnostics and explanatory observations. |
| `github` | Optional `{run_id, run_attempt}` linkage captured in GitHub Actions. |

The exact state spellings are:

| State | Interpretation |
| --- | --- |
| `already_present` | The exact version was observed without needing this attempt to publish it. |
| `published` | Publication completed and the exact version was observed. |
| `would_publish` | Dry-run work for a missing version. |
| `missing` | The requested version remains absent. |
| `unknown` | Availability could not be established. |

A dry run never sets `complete` to true. Use the outcome and command diagnostics
together; unknown queries and partial failure are not reduced to successful
skips. Each attempt writes a new outcome path.

## Derived batches and later outcomes

GitHub reconciliation emits new platform batches linked to the original
manifest. A batch fixes the native target, package versions, executable names,
tags and actual peeled tag commits. Its stable `batch_id` binds that content;
each outcome routing entry supplies `target`, relative `path` and `batch_id`.
Those observations do not get written back into the original manifest.

Native execution refreshes remote completeness before building. Later
reconciliation can emit another batch for remaining work without mutating an
older batch. Binary outcomes link both the publication and batch identities.
Phase outcomes include optional `github: {run_id, run_attempt}` metadata, which
the workflow preserves across artifact transport.

An older successful binary receipt cannot satisfy newer GitHub evidence of
missing assets. This remains true when the new batch contains exactly the same
requests and therefore has the same `batch_id`. Its receipt must be at least
as new as the selected GitHub reconciliation.

Consume these artifacts through the matching tool/action interface rather than
writing them manually. Missing or incompatible artifacts require explicit
recovery; they are not a valid empty work set.

## Reporter job results

`publish report --jobs` reads a JSON object with fixed keys and GitHub job-result
strings:

```json
{
  "prepare": "success",
  "registry": "success",
  "github": "success",
  "binaries": "success"
}
```

This demonstrates the format, not a default successful state. Supply observed
results from the current workflow; `binaries` is the binary matrix job result.
Do not omit failed/skipped jobs, rename keys after your caller's display names,
or synthesize success from existing assets.

The reporter recursively reads `outcome.json` files beneath `--outcomes`,
retaining their artifact subdirectories and run/attempt linkage. It compares
the latest applicable receipts with current job results and the original
manifest. Old successes cannot hide failed jobs or newer missing-asset evidence.

`--output` names the new Markdown report. The command can report and file a
failure issue without the publication manifest, but always marks that case
incomplete. `--no-issue` suppresses issue writes, not GitHub-context validation
or incomplete-delivery failure.
