# Git workflow

This chapter covers conventions for working with git and GitHub pull requests.

For repository-specific action release coordination, see
[paired PR presentation](action-releases.md#paired-pr-presentation).

## Creating GitHub pull requests

When creating PRs with `gh pr create`, do not pass the `--body` flag with an
inline string because PowerShell mangles backticks and special characters.
Instead, write the PR body to a temporary file and use `--body-file path/to/file.md`.

## Check waiting and merge-queue readiness

Distinguish required checks from optional advisory checks using the applicable
GitHub rules, not a check's name or duration. Preserve required validation, review
and approval obligations and any verification explicitly requested by the user.
Do not use "every check has finished" as a readiness condition.

Wait for an optional check only when its result could plausibly change the
readiness decision for the actual diff. Benchmark comparisons are normally
low-signal for documentation or test-only changes that leave benchmarked code,
dependencies, build settings and benchmark machinery unchanged. Changes to those
inputs can make benchmark results relevant, including changes outside production
source files. Assess the affected behavior and the check's scope, not filenames
alone. Being queued, slow or asynchronous does not itself make a check irrelevant.

When only low-signal optional checks remain pending, do not delay the ready-for-review
handoff or keep polling solely for their completion. If merge-queue submission is
already authorized and otherwise allowed, submit without waiting for those checks;
let them continue asynchronously. This policy does not grant merge authority or
bypass GitHub's required checks, reviews or queue rules.

Briefly identify any optional results not awaited and the diff-based reason in
the existing handoff, without claiming they passed. Inspect available failures
and actionable findings even from optional checks, and handle relevant later
results during normal authorized follow-up. Low expected signal is not a reason
to dismiss an observed problem, cancel workflows or weaken validation.

For Codecov verification-key import failures, follow the
[workflow policy and troubleshooting in the Triage guide](triage.md#codecov-verification-key-import-failures).
Skipping uploader executable verification does not relax coverage targets or authorize
treating a failed upload as passed.

### Queue attempts and release freshness

PR checks and merge-queue checks validate different commits. The queue validates
a candidate combining the PR with its current target. Its failures may not appear
in the PR head's check rollup. A passing PR `required-checks` result does not
supersede a failed queue attempt.

Before declaring a PR ready, submitting it to the queue or retrying after removal:

* Inspect live queue membership, recent queue events and the applicable
  **Merge queue validation** runs triggered by `merge_group`. Associate runs with
  the PR and their tested head/base; queue candidates have their own commit IDs,
  so filtering only by the PR head can miss them.
* Read failed jobs and their diagnostics. Establish that each relevant failure
  is fixed and verified, is an identified transient failure suitable for retry,
  or no longer applies to the candidate. A newer commit alone is not evidence
  of a fix. Do not repeatedly enqueue an unchanged candidate with an unexplained
  failure or attribute queue removal to optional checks without evidence.
* Refresh actual release history and the intended merge target, and compare them
  with the version plan's inputs. Another merged PR can consume a proposed
  package version without creating a Git conflict. If either input changed,
  follow [release-plan recovery](release-versioning.md#release-branch-movement-during-planning)
  and revalidate the complete release plan before submission. `MERGEABLE` and
  green checks against an older base do not establish version readiness.

Report readiness for queue submission, confirmed queue membership and completed
merge as distinct states. Successful enqueueing establishes only queue membership;
the queue candidate still needs its required validation.

## Addressing pull request review comments

When addressing PR review comments, reply to each comment thread with the
disposition (what you did to address it) and mark the thread as resolved after
pushing the commit that addresses it.

## Version increments

A pull request that changes a package's released content must increment that
package's version. The increment *is* the release: merge publishes. Do not
increment casually, and do not leave released-content changes without an
increment.

Run the `increment-versions` skill to propose and apply levels. The author may
raise a level above the `cargo-semver-checks` floor; they may not lower one.
The skill decides and applies every change level without a separate human
approval request. Human review and merge of the complete pull request remain
the final approval, including the release it causes.

See [release-versioning.md](release-versioning.md).

### Version/release plan section

Every pull request description must contain a clearly identified
**Version/release plan** section.

Base the section on the final expanded plan and current release evidence, not
just the packages directly edited. Use one row per version group and per
ungrouped package, naming every member reached by the expanded plan. Show each
package's previous version at its release anchor, its proposed version, the
substantive change level, and the reason. When members have different previous
versions, make their individual movements clear.

Explain necessary dependent and group movements, including packages moving only
to keep a group aligned, public-dependency compatibility propagation, and
dependency requirement rewrites. Include pending increments already present in
the PR rather than describing only the latest application of the skill. State
the reason for a level above the SemVer floor; identify mechanical realignment
without inventing a consumer-facing change.

If there are no released-content or version changes, state that explicitly
instead of omitting the section. Identify new packages separately with their manual
bootstrap versions, higher versions intended for the first automated release, and
the maintainer handoff. Manual bootstrap publication and Trusted Publishing setup
must precede the first merge; that merge performs the second publication. See
[the first-publication procedure](../RELEASING.md#first-publish-of-a-new-crate).
An empty increment plan does not mean there is nothing to release.

Refresh the evidence, plan, and section when the source, release history, merge target, group
membership, or decisions change. The section describes the final current PR,
not the history of intermediate plans. Keep it focused on release decisions,
not a generic changed-file list or a validation log. The PR is ready for human
review only when the section matches the release it proposes.
