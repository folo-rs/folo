# Git workflow

This chapter covers conventions for working with git and GitHub pull requests.

For repository-specific benchmark-action coordination, see
[paired PR presentation](benchmark-action-releases.md#paired-pr-presentation).

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
instead of omitting the section. Identify first-publication packages separately
with their initial versions and the maintainer handoff; an empty increment plan
does not mean there is nothing to release.

Refresh the evidence, plan, and section when the source, release baseline, group
membership, or decisions change. The section describes the final current PR,
not the history of intermediate plans. Keep it focused on release decisions,
not a generic changed-file list or a validation log. The PR is ready for human
review only when the section matches the release it proposes.
