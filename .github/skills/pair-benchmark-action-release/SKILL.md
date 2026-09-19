---
name: pair-benchmark-action-release
description: Run after increment-versions in folo-rs/folo to coordinate the repository-specific benchmark-action release. Use after version planning or reassessment, when preparing the paired action PR, or when following its publication dependencies.
---

# Scope

Run this repository-specific extension after
[`increment-versions`](../increment-versions/SKILL.md) has verified the complete monorepo
release plan. It coordinates the corresponding PR in `folo-rs/cargo-bench-history-action`
under the [benchmark-action release policy](../../../docs/benchmark-action-releases.md).
It consumes the general skill's evidence; it does not decide or apply crate increments.

Run the fast relevance check before inspecting the action repository or PRs.
Read the action repository's contribution and release instructions before changing it.
Its manifest and release policy determine the pinned tool set and independent action version.
Completing this skill authorizes neither merging either PR nor publishing packages, tags or
action releases. Missing access or incomplete evidence is an explicit handoff blocker.

# Inputs and working files

Use the completed `increment-versions` run's absolute `WORK_DIR` and `VERIFY_DIR`.
Stage 1 needs only the verified report. If it returns `true`, read the resolved
`preview/plan.json`, retained pending-release assessments, and Stage 7
verification report and compatibility evidence, together with the current
**Version/release plan** section, prepared locally if the PR does not exist yet.
An empty applied plan does not mean there are no pending releases. If evidence is missing or
no longer describes the current source, release baseline, groups or decisions, return to
`increment-versions` before choosing action pins. Publication follow-up uses the verified
paired release and current action manifest; it does not reserve another crate increment.

Keep `{{WORK_DIR}}\action-pairing.md` as the untracked handoff, referencing those artifacts
rather than maintaining another package/version inventory. Record command diagnostics as
they occur and preserve the last verified state when a command fails. Prepared PR descriptions
also belong under `WORK_DIR`; source edits remain in their owning checkouts. Commit none of
these working files.

# Placeholders

| Placeholder | Description |
|-------------|-------------|
| `WORK_DIR` | The absolute untracked evidence directory from the completed general version-planning run. |
| `VERIFY_DIR` | That run's separate absolute Stage 7 verification directory. |
| `ACTION_MANIFEST` | Optional local action `release.json`, for bootstrap or proposed pins not on the default branch. |
| `ACTION_BRANCH` | The published action branch containing this verified release's pins and action version. |
| `ACTION_TITLE` | The action PR title describing the release change. |
| `ACTION_BODY`, `MONOREPO_BODY` | Absolute paths to the complete prepared PR descriptions under `WORK_DIR`. |
| `ACTION_PR`, `MONOREPO_PR` | The discovered or created PR numbers or URLs in their respective repositories. |
| `ACTION_RUN` | The workflow run for the current action PR revision whose required installation check must be rerun after publication. |

# Stage 1: Check relevance cheaply

Run the repository-specific checker against the completed generic skill's verified report:

> just benchmark-action-pairing-needed "{{VERIFY_DIR}}\report.json"

For initial action bootstrap or an explicitly selected proposed manifest:

> just benchmark-action-pairing-needed "{{VERIFY_DIR}}\report.json" "{{ACTION_MANIFEST}}"

It prints one Boolean on stdout. On `false`, record that no pinned package is pending release
in `{{WORK_DIR}}\action-pairing.md` and finish; do not discover PRs or require an action checkout.
On `true`, continue below. A pin already updated in a paired PR still counts because publication
and installation follow-up remain necessary. On any nonzero exit, stop and report the diagnostic:
missing or malformed inputs cannot authorize a no-op.

The helper skips manifest access when there are no pending releases. Otherwise it reads the
authoritative action tool list, including fixture tools, without cloning the repository.

# Stage 2: Discover the pairing and affected pins

Discover the action repository and existing open PRs before creating a pairing:

> gh repo view folo-rs/cargo-bench-history-action --json nameWithOwner,url,defaultBranchRef
>
> gh pr list --repo folo-rs/cargo-bench-history-action --state open --limit 100 --json number,url,title,body,headRefName

Stop and record an access error in `{{WORK_DIR}}\action-pairing.md` if either command fails.
Inspect the returned PR descriptions and branches for work linked to this monorepo release;
reuse the matching PR rather than creating another. If the result reaches the requested limit,
narrow the search or continue discovery before treating a pairing as absent.

Locate the current release manifest using the action repository's own instructions; do not
assume a filename. Inspect the existing paired PR's proposed manifest as well when refreshing
a pairing. If the repository needs its initial manifest and runtime, record that bootstrap
pairing as outstanding; an absent manifest does not establish no affected pins. Resume the
pin comparison when the initial paired PR defines the manifest.

Compare all pinned monorepo packages, including test-only tools and workflow helpers, with
the complete verified pending release set. Include retained increments, first-publication
packages, dependency/group effects and version-only movement, not merely entries newly
applied by `increment-versions`. Private implementation packages matter through the pinned
binary versions they move. Do not infer a released predecessor when the evidence has none.
A pin already updated in the existing paired PR still needs its publication/check follow-up.

If no manifest pin is affected, record that evidence and finish with the Stage 5 handoff;
do not create an unrelated action PR.

# Stage 3: Update the action and cross-link the PRs

When a pinned tool moves, update the action manifest to the final exact tool versions and
choose the appropriate action-version increment under that repository's release instructions.
Action and tool versions are independent; a pin-only change still needs an action release.
When refreshing an existing pairing, reassess its pending action version rather than
blindly incrementing it again. Use the action repository's normal branch and change-validation
procedure.

Prepare complete PR descriptions under `{{WORK_DIR}}`, following
[paired PR presentation](../../../docs/benchmark-action-releases.md#paired-pr-presentation).
If the monorepo PR does not exist yet, record the remaining cross-link obligation and defer
commands requiring `MONOREPO_PR` until its creation.
After publishing the action branch, use the applicable create or update command:

> gh pr create --repo folo-rs/cargo-bench-history-action --head "{{ACTION_BRANCH}}" --title "{{ACTION_TITLE}}" --body-file "{{ACTION_BODY}}"
>
> gh pr edit "{{ACTION_PR}}" --repo folo-rs/cargo-bench-history-action --body-file "{{ACTION_BODY}}"

Capture the created PR URL, then cross-link both descriptions, preserving their existing content
and Version/release plan sections. Update the monorepo description and read both PRs back:

> gh pr edit "{{MONOREPO_PR}}" --repo folo-rs/folo --body-file "{{MONOREPO_BODY}}"
>
> gh pr view "{{ACTION_PR}}" --repo folo-rs/cargo-bench-history-action --json url,body,headRefName
>
> gh pr view "{{MONOREPO_PR}}" --repo folo-rs/folo --json url,body,headRefName

On a failed write, stop and preserve the returned diagnostic and last verified pairing state
in `{{WORK_DIR}}\action-pairing.md`; do not report successful coordination or create another
PR blindly. Verify that both returned descriptions contain the correct reciprocal links.
If readback fails, the pairing remains unverified. Fulfill any deferred cross-link obligation
when the monorepo PR is created.

# Stage 4: Follow publication and the required installation gate

Keep the pair current after any reassessment, including release-baseline recovery. The
monorepo merges first under its normal authorization and checks. Creating the paired PR is
a monorepo readiness obligation; passing the action's required `install-tools` check gates
the action merge, not the monorepo merge that starts asynchronous dependency publication.

The check must really install the exact manifest packages and promised archives as specified
by the [installation gate](../../../docs/benchmark-action-releases.md#publication-and-required-installation-gate).
Expected early failure while publication is pending remains merge-blocking for the action.
Source dogfooding, source fallback for a promised archive, or an existing binary cache cannot
substitute for this required check. Unlike an advisory check, it must pass before action merge.

The paired PR's author follows publication and explicitly reruns the failed check after all
required packages and archives are available:

> gh run rerun "{{ACTION_RUN}}" --repo folo-rs/cargo-bench-history-action --failed
>
> gh pr checks "{{ACTION_PR}}" --repo folo-rs/cargo-bench-history-action --required

Record the actual result for the current action manifest, including pending or failed
publication dependencies; a rerun request is not a passing check. A failed rerun request
is a handoff blocker. Preserve the pending follow-up when publication has not completed;
do not merge or publish anything to unblock it from this skill.

# Stage 5: Hand off the verified state and blockers

Keep `{{WORK_DIR}}\action-pairing.md` current and summarize its disposition to the caller:
the affected pins and verified reciprocal PR links, the action-version decision, why an
existing PR was reused or a new one was needed, the next publication/check follow-up and
any access, PR-creation or verification blocker, or the evidence for no affected pins.
Reference the verified release plan rather than duplicating its version inventory.

When a summary is posted as a GitHub comment, place execution diagnostics in a collapsible
section. The handoff does not grant merge or
publication authority.
