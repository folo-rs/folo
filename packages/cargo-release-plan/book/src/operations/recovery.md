# Recover incomplete work

Recover the requested release, not whatever versions happen to be current when
you notice the failure. Keep the original manifest, run identity, batches and
attempt outcomes while inspecting the failure.

Successful registry uploads, established tags and complete asset pairs are not
rolled back. A retry observes them and completes missing work.

Retain phase receipts as separate `outcome.json` artifact subdirectories with
their GitHub run/attempt metadata. An older successful binary receipt must not
replace work requested by newer missing-asset observations, even if both batches
have the same identity.

## Triage checklist

1. Identify the original run, publication identity, source commit and failing
   phase.
2. Read package-specific diagnostics, including cleanup and reporting failures.
3. Distinguish unknown remote state from confirmed missing state.
4. Confirm the original manifest and required batches remain available.
5. Correct the specific source, setup, permission or remote-state blocker.
6. Retry the original run or phase with the same intent and a new outcome path.
7. Verify all required phases; an independent success does not erase another
   release's failure.

## Registry and binary failures

| Failure | Recovery |
| --- | --- |
| Throttling or transient registry unavailability | Let bounded retries finish; when exhausted, retry original intent after service recovery. |
| Cargo fails after an upload | Requery exact-version availability. Do not assume failure means the version is absent. |
| OIDC exchange or registration failure | Verify caller repository, entry workflow and environment registration. Do not add a PAT fallback. |
| Missing or inconsistent lockfile | Correct the source through reviewed planning. Publication never regenerates resolution to make an upload pass. |
| Missing checksum or archive | Reconcile the existing release; repair the incomplete pair together. |
| Deterministic compiler failure | Fix the native build prerequisite or source defect rather than repeatedly retrying an unchanged command. |
| Query failure | Restore access and repeat the observation. Do not turn an unreadable release into an empty asset inventory. |

If a source defect needs correction, it belongs in a reviewed forward release.
Do not edit an immutable manifest to point an old version at new content.

## Missing tag after a newer version merges

Consider publication source `A` requesting `widget-cli` version `2.0.1`. While its
registry phase runs, source `B` merges `2.0.2`. The older run still needs
`widget-cli-v2.0.1`, but the current release-branch candidate no longer carries
that version.

Workflow queuing does not serialize those merges. The publisher does not relabel
`B` as `2.0.1`, prevent further merges or acquire broader tagging authority.
Instead:

- It skips this release's tag-dependent work.
- Independent releases and valid native batches continue.
- The reconciliation phase and workflow remain failed.
- The failure issue identifies the package/version, exact missing tag, original
  publication source SHA, conflicting branch version when applicable, failure
  reason and original run.

### Operator tag recovery

An operator with the necessary rights verifies the recorded source and creates
the missing tag **at the original publication source**, then retries the
**original failed run**.

The commands below are operator actions, not automatic repair instructions for a
version-planning agent. Replace the source and run placeholders from the failure
report, verify repository identity, and confirm the source contains the requested
package/version and release inputs:

```powershell
$Repository = "example/widgets"
$Tag = "widget-cli-v2.0.1"
$Source = "<full-original-publication-source-SHA>"
$RunId = "<original-failed-run-id>"
git fetch origin --tags
git show --no-patch --format=fuller $Source
git ls-remote origin "refs/tags/$Tag" "refs/tags/$Tag^{}"
```

Only when the tag is absent locally and remotely, create and push it:

```powershell
git tag --annotate $Tag $Source --message "Release $Tag"
git push origin "refs/tags/$Tag"
gh run rerun $RunId --repo $Repository
```

Never force-update an existing tag. If a tag already exists but points to
unexpected source, stop and investigate its identity rather than replacing it.

The retry recognizes the valid manually created tag through its existing-tag
path. It does not repeat the moving-candidate selection used only for missing
tags. It can create the missing GitHub release and generate the formerly blocked
native batches, which build from the tag's actual peeled commit. Existing
registry versions and completed assets are retained.

A new run against the latest branch tip is not a substitute for completing
`2.0.1`. Manual tagging resolves this blocker only; unrelated failures remain.

## Missing or expired artifacts

A missing manifest or required batch is an error, not an empty release. Do not
reconstruct a receipt from a later checkout or edit an outcome into success.

The final `publish report` operation still runs when the manifest is unavailable:
omit `--publication`, retain the available outcomes and current job results, and
report the failure in the original GitHub run context. It writes Markdown and
can create the failure issue, but cannot declare the release complete.

If retained artifacts are unavailable, choose the original immutable source
explicitly and use a compatible tool to prepare a **new** publication manifest.
This is a separate recovery invocation with new evidence and fresh remote
validation, not a continuation of a missing artifact. Preparation still requires
clean source and its committed configuration and lockfile.

If that source is unavailable or its artifacts require another schema/tool
version, restore the retained evidence or select the matching tool deliberately.
Never fall back silently to the latest source.

## Release-branch movement

This is a pre-merge planning problem, separate from publication recovery.
A newer release baseline can consume an already selected version and invalidate
prepared evidence.

Refresh `release-context` and compare its `release_base` with the saved baseline
to establish whether branch movement caused the failure. Then:

1. Identify version, requirement and lockfile edits generated by the superseded
   planning run.
2. Undo only those generated edits, preserving source changes, independent
   manifest edits and upstream work. No rollback is needed if nothing was
   applied.
3. Integrate the current release branch using the repository's normal policy.
4. Freeze a fresh baseline, prepare and preview again in new evidence directories.
5. Refresh compatibility evidence and the PR's Version/release plan section.

Do not increment stale proposed versions manually, reset the whole worktree or
revert a mixed-purpose commit. Resolve genuine conflicts deliberately. The new
assessment may retain an adequate pending increment; rerunning is not itself a
reason to increase versions.
