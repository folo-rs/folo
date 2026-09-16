# Scheduled validation

## Purpose

Keep ordinary PR and merge-queue validation fast while running complete standard and deep
validation nightly. GitHub Actions runs the checks and reports failures. A human or personally funded Local
Copilot App session triages each report into independently actionable issues and
repairs them through ordinary pull requests.

```text
Nightly or manual standard + deep checks
              |
              v
Readable failed-run issue
              |
        Human or App triage
              |
              v
Problem issues, reusing existing ones where appropriate
              |
        Human or App repair
              |
              v
PR -> checks and review -> human approval and merge
```

GitHub issues, comments, labels, branches and PRs carry every handoff. A person
reading an issue can understand the problem, ownership and next action without
private files, encoded records or another agent's conversation. Templates guide
authors; human-written issues with equivalent information work on the same terms.

The [workflow design](../.github/workflows/design.md) and
[implementation guide](../.github/workflows/implementation.md) cover hosted
execution. [Testing](testing.md) defines mutation and Miri quality requirements.
The local shallow and deep validation commands retain their separate meanings.

## Checks and readable failure reports

**Standard validation** runs ordinary required PR checks and full-scope checks on main pushes.
**Merge queue validation** runs only full-workspace dev Clippy, formatting and version readiness.
**[Deep validation](../.github/workflows/deep-validation.yml)** reuses the complete standard
workflow, including its full package, tooling and platform scope, and runs the complete
Miri platform matrix, many-seed Miri cases, mutation shards, careful checks,
release-profile Clippy, release builds, example execution, dependency default-feature policy checks,
feature-powerset compilation, unused-dependency checks and ARM64 tests with benchmark
smoke checks nightly on main. Standard coverage includes native tests and coverage,
documentation and doctests, minimum-dependency compilation, external-type checks, tooling checks,
release validation and Azure backend tests. Every nightly run executes the checks; previous
success does not skip a night. Ordinary dependency/build caches remain available.

The scheduled matrix invokes the same `just miri`, `just miri-harder`, `just mutants`
and `just careful` recipes available to developers, with the relevant package and
shard arguments. Each summary records the exact Just command. Check behavior and
exit status belong to those recipes; the scheduled wrapper only captures output
and formats diagnostics.

A checker finding fails its Actions job and the validation run. Missed mutations,
mutation timeouts, setup failures and incomplete execution remain failures.
Independent matrix jobs continue after another job fails, and diagnostic uploads
run even on failure.

Planning, standard and deep checks, and failure reporting belong to the same main-only workflow.
Main pushes do not cancel its standard checks. Publication remains independent of post-merge
validation, so detection does not prevent a regression from being released.
The report job has ordinary issue-write permission and can report planning or
toolchain setup failures before checker artifacts exist.

Each failed run attempt gets a normal `scheduled-run-failure` issue titled
**Scheduled validation failed on &lt;UTC date&gt;**. It contains the workflow and
run/attempt link, tested commit and start date, unsuccessful jobs with their
conclusions and direct links, observed error summaries and useful diagnostic
excerpts. It links full logs and tool-generated artifacts and explains missing
diagnostics or checks that never ran. The reporter describes observations, not
inferred root causes.

Reports start with `[Copilot speaking]`. Bounded per-job excerpts may continue in
readable Markdown comments, not API response blobs or encoded pages. Every
unsuccessful job retains diagnostic links, and clipped text has an omission notice.
Useful failure details remain on GitHub after Actions logs expire; successful-job
inventories and whole logs do not belong in issues. Diagnostic verbosity does not
create an unlimited sequence of comments.

The visible run URL and attempt identify a report. Reporter retries search open
and closed reports for that attempt before creating one, then publish only missing
sections without rewriting completed snapshots. Writes are paced; explicit rate
limits receive bounded backoff and ambiguous outcomes are checked for persistence,
not blindly replayed. Each failed rerun gets
its own report; a successful rerun neither files a failure nor silently closes
older reports or problems. If reporting fails, the report job's failure remains
visible in **Deep validation**. Resolve an ambiguous duplicate with an ordinary
linked duplicate explanation.

## Running checks manually

Manual checks need permission to run repository Actions, not Local App setup,
models or repair ownership. Open **Actions -> Deep validation -> Run workflow**
on **main**, or run:

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
gh workflow run deep-validation.yml --ref main
```

This command has no placeholders or workflow inputs. Every run tests the full
scope at main; it cannot select a PR, branch, source commit, package or subset of
checks. Inspect the resulting run's tested commit, jobs and diagnostic artifacts.
A nonzero command exit is a dispatch failure, not a successful check.

If reporting fails, reopen that **Deep validation** run and use the standard
Actions rerun controls. **Re-run failed jobs** may also rerun failed checks.
To request a rerun of the report job through the CLI, first find its database ID:

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
gh run view "{{RUN_ID}}" --json jobs --jq '.jobs[] | {name, databaseId}'
gh run rerun --job "{{JOB_ID}}"
```

| Placeholder | Value |
|---|---|
| `RUN_ID` | The failed Deep validation run's ID. |
| `JOB_ID` | The report job's `databaseId` from the first command, not a number inferred from its browser URL. |

Inspect the first command's output before submitting the second. Reruns follow
GitHub's job dependency rules; do not assume only reporting executes. Check the
result in the same workflow. Failed-run reporting and successful reruns do not
themselves close triage reports or problem issues.

## Checking repairs locally

Use the existing [build commands](build-and-tooling.md) at the PR commit:
`just package="cpulist" validate-local` runs shallow validation, and
`just package="cpulist" validate-deep-local` runs deep checks on the current
platform. Use targeted just recipes for the particular deep checks relevant to a
repair, and use WSL when Linux execution is required.

To reproduce scheduled capture locally, invoke
`scripts/scheduled/Invoke-ScheduledCheck.ps1` with the check JSON and tested commit.
It prints a unique result directory under the system temporary directory. An explicit
`-OutputDirectory` must be empty and outside the source checkout, keeping live logs
and mutation artifacts out of source copies. Retain the directory for diagnostics.

The main-only hosted workflow is not PR-head validation evidence. Apply the
[platform support policy](build-and-tooling.md#platform-support-and-validation):
ARM64 validation is best-effort and may be skipped without separate approval when
the repair should logically work there, especially when the same corrected logic
passes on other platforms. Record the skipped scope and rationale, not an executed
pass. Missing ARM64 validation alone does not warrant `needs-human` or prevent a
ready-for-review handoff.

If a required platform outside that exception is unavailable locally, disclose the
missing scope and add `needs-human` for the needed decision. Human review may
resolve that limitation; it is not a successful check. Do not introduce hosted
scope selection to bypass it.

## Triage

Use the [scheduled-triage skill](../.github/skills/scheduled-triage/SKILL.md) or the
same workflow manually. Select open `scheduled-run-failure` issues oldest first,
claim the report, and process reports sequentially. Read the report, relevant
jobs/logs and source, then search relevant open and closed issues and related PRs,
including human-filed issues without automation labels.

Separate independently actionable problems, not jobs. A dependency download
failure affecting several jobs is one problem; unrelated defects in one job are
separate problems. A failed prerequisite explains work that never executed, not
defects in every dependent package. Compare actual problems rather than identical
log text or generated fingerprints.

Create or update a normal problem issue with `scheduled-finding`, including when
reusing a human issue. It needs an observed failure and affected scope, known cause
or explicit uncertainty, useful diagnostics and report/job links, reproduction
steps with applicable toolchain/target/seed details, and acceptance criteria.
Triage need not finish the repair investigation or prescribe a speculative patch.

When supported by the evidence, distinguish packages likely to need repair edits
or version movements from packages merely affected by a failed check. Note known
version-group or dependent-package effects and link any prerequisite repair,
explaining why it is needed. Mark estimates and unresolved scope explicitly;
these ordinary issue notes help intake assess overlap without requiring a release
plan, a mandatory field or a speculative dependency.

Repeated unresolved failures update the existing problem with links and materially
new evidence. A supported recurrence after a fix can reopen the issue; a run of
pre-fix code is not a recurrence. Link uncertain duplicates rather than
consolidating unproven matches. Do not retarget an owned repair without reconciling
the changed diagnosis with its owner.

Add `needs-human` for permission, policy or external-intervention blockers and
explain the needed action. An infrastructure recovery can resolve an issue with
an explanation and applicable successful rerun, without an artificial source
patch. An unexplained intermittent failure is not fixed merely because a retry
passes.

Post a concise mapping of the report's failures to problem issues or explained
non-actionable outcomes. An intentional operator cancellation can be dismissed
with an explanation. Close the report only after every failure is accounted for
and the linked issues contain the handoff information. Missing decisive evidence
keeps it open with a concrete blocker. Report closure means triage is complete,
not that its problems are fixed.

## Ownership and handoff

`scheduled-run-failure` identifies triage work; `scheduled-finding` identifies the
repair backlog. Assignment on an open issue records ownership, not whether a worker
is currently executing. `needs-human` records a human-action blocker. Issue state
and linked PRs supply the remaining lifecycle.

Before working, read current discussion, assignees and linked PRs. Claim the issue
by assigning the responsible GitHub user and posting an ordinary comment, for example:

> [Copilot speaking]
>
> Claimed for repair by @owner in App session "Repair cpulist Miri failure".
> Working branch: `<branch>`. I will link the pull request here.

Use actual identities, and link the GitHub branch and PR when available. An App
session link is helpful but not required for another person to understand the
work. Session identity distinguishes agents sharing an account; an assignee alone
does not authorize adopting another worker's task.

Reread claims after posting and before work. If claims collide, the earlier
unreleased claim takes precedence and the other worker withdraws without removing
the winner's assignment. This is a human collaboration convention, not
an atomic locking service.

Post substantive progress, blockers, releases and handoffs, not heartbeats. A
blocked owner retains the claim. Elapsed time, machine downtime or an idle session
does not authorize takeover. The owner or a human maintainer must explicitly
release or transfer work, accounting for unpublished changes before replacing an
executor. `needs-human` blocks continuation until its stated requirement is
satisfied or evidence or a human correction establishes that it was not a
blocker. The owner corrects an unsupported blocker in the discussion and removes
the mistaken label without dismissing separate unresolved human requirements.
This does not waive checks. To explicitly release a claim, remove the responsible
assignee and record the release or handoff in discussion without disturbing other
workers. Issue closure needs no assignment cleanup; assignees on closed issues do
not indicate ongoing work.

## Repair and PR completion

The [scheduled-intake skill](../.github/skills/scheduled-intake/SKILL.md) coordinates
repairs. It follows existing claimed issues and PRs first, then starts at most one
new repair session per invocation, subject to the
[repair-session limit](#repair-session-capacity-and-cleanup) after completion cleanup
and the [package-overlap policy](#package-overlap-and-stacked-repairs).
Only open findings are repair candidates; closure ends automatic repair follow-up,
including follow-up of linked PRs. Known sessions linked to closed findings remain
in scope for completion reconciliation and safe archival. Keep the issue open
while its repair is ongoing. Closing it with an open PR requires an explicit
disposition of that PR, not continued automatic intake follow-up.
The [scheduled-repair skill](../.github/skills/scheduled-repair/SKILL.md)
works on one issue in its native issue/PR-linked Local App session.

Confirm the failure, implement the correction and create a normal PR with
`Fixes #<issue>`. Follow repository conventions, including `increment-versions`
and the complete current **Version/release plan**. There is no separate version
attestation or managed-repair merge gate.

Normal required PR checks run. The worker also runs relevant deep checks locally
at the current PR commit and records the tested commit, scope, commands, outcomes
and any limitations in a PR comment. Relevant subsequent changes require fresh
local results at the reviewed head; unrelated green checks do not establish the
fix. Reviewers assess these results and limitations alongside ordinary required
checks and version validation.

Follow the same PR through CI/deep failures, conflicts and review feedback. Read
top-level discussion, review summaries and inline threads, including valid
low-confidence agent comments. Follow repository communication policy, including
the exception permitting responses to the original user's own human comments.
Every authored post begins with `[Copilot speaking]`. Request human decisions for
design changes or unsafe ambiguity; do not call blocked work complete.

Apply the [check-waiting policy](git-workflow.md#check-waiting-and-merge-queue-readiness)
to optional GitHub checks. Do not wait for low-signal optional results, such as
benchmark comparisons when no performance-relevant inputs changed, solely to
finish every check. They may continue after a ready handoff; record what was not
awaited and why, without claiming success. Intake does not wake a worker merely
to wait for those results. Repair owners handle actionable findings during their
normal authorized follow-up.

Queued, pending and in-progress checks or automated reviews are normal ongoing
work. Queue age, no assigned runner, absent steps/logs before execution and other
queued repository runs do not establish an outage or a need for human action.
The worker continues requested foreground follow-up while required checks,
relevant optional checks or automated review are pending, waiting between
current-head check and review reads rather than busy-polling. This requires no
per-PR timer, automation or hidden watcher. A delay in results worth awaiting is
not a reason to end that work, add `needs-human` or request a check waiver.

Human blockers require concrete evidence and a specific action outside the
worker's authority, such as a required approval or a diagnosed permission failure.
Diagnose failed execution and pursue authorized recovery before escalating.
Only hand off as ready for human review/approval/merge after current-head required
checks, relevant optional checks and automated review have concluded and actionable
findings are addressed. Relevant local deep verification remains necessary.
If foreground execution is interrupted while results worth awaiting are pending, retain
ownership and the next follow-up action as pending work, not a human blocker.

Repair branches follow ordinary repository conventions. Normal same-repository
and fork job rules apply; branch names do not grant special treatment.

Final approval and merge remain human actions. A merged linked PR closes its
problem issue through normal GitHub behavior. A PR closed without merging does
not resolve the issue: record the disposition and explicitly release or block the
claim. No post-merge confirmation service is needed; later scheduled failures are
triaged normally.

### Repair-session capacity and cleanup

The maximum incomplete repair sessions defaults to `5`. An operator can override
it with a nonnegative integer in the intake invocation or saved repair automation
prompt. `0` pauses new admissions while preserving follow-up and cleanup. Invalid
or conflicting values require clarification, not a silent fallback. The limit
applies to this repository's scheduled repairs across intake invocations and
parent sessions; unrelated human work, triage/coordinator sessions and other
repositories do not consume its capacity.

Count distinct repair sessions, reconciling native issue/PR links with GitHub
ownership comments. Running, paused, idle, unavailable and `needs-human` repairs
remain incomplete. A ready PR waiting only for human review or merge still counts
until merged or explicitly abandoned. Neither an idle status nor an archive flag
proves completion. Do not double-count a session because both an issue and a PR
refer to it.

Every intake invocation follows existing work and reconciles completion before
checking capacity, even when at the limit or the open backlog is empty. Inspect
known sessions linked to closed issues and merged or closed PRs for cleanup,
without scanning the closed backlog for new work. A merged PR or an explicitly
abandoned repair with its PR closed and claim released establishes a final
disposition. A repair without a PR needs a documented resolution or explicit
abandonment. Closing an issue while its PR remains open does not make the session
archivable. Already archived sessions with a verified final disposition need no
further cleanup.

Finish remaining actionable handoff work through the existing owner. Archive a
finished session only after verifying it has no unpublished or unmerged work to
preserve, open PR, ongoing operation, active Agent merge or attached session
automation. A worker left active after its repair is finished receives a focused
completion-handoff request; verify it has ended before archiving it. Do not force
genuinely ongoing work to finish or discard local changes to free capacity.

Use supported native archival and verify the outcome. `archive_session` is
restricted to sessions created by its caller and cannot archive the caller itself.
Cleanup outside that authority requires the owning parent or operator; report the
needed action and defer new admissions when required cleanup is blocked or
uncertain. Do not substitute session/worktree deletion. Completion handoffs stay
on the issue/PR and in native sessions, not in a private lifecycle registry.

After cleanup, refresh the incomplete count. At or above the limit, continue
existing repairs but do not claim another repair or create a session. The same
gate applies to a replacement executor after an explicit handoff; resuming an
existing incomplete session does not consume another slot. Below the limit,
prioritize handed-off work needing an executor, then the oldest actionable
unclaimed finding, and start at most one new session. Refresh capacity immediately
before opening a new session or claiming a new repair in an existing session.
Creating the admitted executor occupies its slot; claiming and starting that same
executor do not require another slot. Incomplete discovery or uncertain
ownership/completion defers admission rather than implying free capacity. Keep
intake invocations nonoverlapping; these observations are not an atomic reservation
or a financial cap.

### Package overlap and stacked repairs

Concurrent repairs of the same package can independently choose the same version
increment. Intake reduces this avoidable synchronization work by deferring a new
repair when its likely edited or version-moving packages overlap an incomplete
repair. Waiting for checks, human review or merge does not release that scope.
This is a best-effort scheduling heuristic, not a package lock or a replacement
for normal version validation.

Use current PR **Version/release plan** sections, changed paths and substantive
issue/owner notes. Include known version-group members and dependent releases, not
just directly edited packages. Distinguish likely repair targets from a check's
broader affected scope; an infrastructure failure or unreleased documentation edit
does not imply every tested package needs publishing. A missing release plan is
not an empty package set. Make a bounded read of relevant source or manifests when
useful, without preparing Rust or computing exhaustive release plans in intake.
Defer plausible overlaps whose scope is unresolved, but uncertainty alone is not
a repository-wide lock. Continue scanning for the oldest eligible independent
finding rather than stopping at the first deferred issue.

A new repair may instead form a stacked PR when it naturally depends on an
existing repair's changes. Sharing a package or wanting a different version is
not a dependency. The proposed parent must have an open PR with committed, pushed
changes and a current release plan whose increments are present at its head.
Published progress and PR evidence must establish a settled scope and release
plan; unresolved development likely to change them is not a suitable base.
Checks may still be running if they do not reveal such unresolved work.

Use one ordered chain, extending its current top rather than creating competing
siblings. Verify that the whole prerequisite chain is suitable and contains every
known overlapping repair; a stack does not excuse a collision with unrelated
work. Record the dependency reason, parent issue/PR, branch and exact head commit
on GitHub. Recheck the parent and package scope immediately before admission and
again before the worker edits. If the parent merged, reassess against current main;
if it moved, was abandoned or is no longer suitable, defer or reconcile through
the existing owners rather than silently changing the base.

Each layer has its own session, claim, branch and PR, consumes a normal incomplete
slot, and counts toward the one-new-session-per-invocation limit. The coordinator
creates only the admitted upper layer from the verified pushed parent branch,
using `create_session` with an explicit `base_branch`. Its ordinary issue ownership
comment identifies the session until its own app-native PR supplies the native
link. Do not open a duplicate issue session to attach it. The Local App's bundled
`pr-stack` skill supplies native stack inspection, creation/extension and
synchronization mechanics; it is an App prerequisite, not a repository-local
skill. Confirm it is exposed before admitting a stacked layer. If unavailable,
defer stacked admission, disclose the missing prerequisite and continue to
consider independent repairs without installing a substitute. This skill does
not authorize extra workers, modifying another owner's branch or merging.
Where native registration is unsupported, an explicit dependent-PR chain retains
ordinary bottom-to-top synchronization in the owning sessions without requiring
native membership.

The worker keeps release evidence anchored to current main, not an unreleased
parent. It also assesses its own released-content and dependency effects against
the parent: each existing package needing a release for this layer must advance
beyond the parent's version by the level this layer requires. An inherited pending
increment is not that additional increment. Inheritance alone does not justify
a second semantic increment. Required group alignment and dependent releases
still move every target in the expanded plan, including otherwise unchanged
inherited packages. New packages follow first-publication rules. Keep the complete
release plan and explain all parent-to-child movements, including mechanical
movements, separately from release-anchor versions.
Parent changes or merges require refreshed scope, version and relevant validation
evidence even if the child's existing checks are green. Owners publish the
reconciled parent snapshot for later intake runs. Agreed ancestor/descendant
overlap does not block necessary parent fixes; coordinate downstream updates.
Normal version checks remain authoritative.

Workers publish likely package scope early and update it when diagnosis or the
expanded release plan changes. Intake records substantive overlap deferrals and
prerequisite links in ordinary issue discussion so later invocations can reassess
them. Package waiting alone does not warrant assignment, `needs-human`, a timer
or repeated comments. No schema, reservation counter or private registry is needed.

## Local App setup and operation

Use separate repository-level **Local** App automations for triage and repair.
Inference uses the operator-selected personal account and model, not GitHub
Actions or a cloud coding agent. Keep one enabled entry per role and avoid
overlapping invocations. Starting at most one new repair per invocation is pacing,
and the incomplete-session limit bounds outstanding repair work, not spending.

Run the checked-in [setup prompt](../.github/prompts/setup-scheduled-remediation.prompt.md)
when installing or updating the entries. It uses `list_projects`, `list_workflows`,
`save_workflow` and the supported native editor. It lists existing entries,
updates the operator-selected disabled ones rather than blindly duplicating them,
and defaults to disabled until enabling is explicitly authorized. Model/effort,
personal account, Local environment, schedule and repair-session limit are ordinary
operator choices. Setup neither installs tooling nor runs an automation.

| Role | Suggested App name | Skill |
|---|---|---|
| Triage | Folo scheduled failure triage | `scheduled-triage` |
| Repair coordination | Folo scheduled repair | `scheduled-intake` |

The coordinator uses native session lookup and `open_issue_session` or
`open_pr_session` to open/resume visible linked sessions, and `create_session` with
the verified parent branch for an admitted stacked layer. It preserves the
existing executor when available. GitHub remains the source of truth; native
runtime metadata locates executors and verifies session cleanup, not repair
correctness or ownership.

Empty scans still reconcile known repair sessions for cleanup, then exit without
posting empty-scan updates or preparing Rust/WSL. Routine waiting does not
trigger duplicate worker turns or heartbeat posts and does not cancel an active
worker's requested foreground follow-up. Subsequent intake runs read current
checks and reviews and route actionable results to the existing owner. New
decisions or other material information can resume blocked work. Follow-up uses
the repository repair automation, never per-PR timers or hidden watchers.

A paused machine leaves the backlog intact. After an explicit handoff, a human
or another machine can resume from GitHub without copying coordination state.
There are no Local state stores, enrollment files, profiles, dispatch tokens,
persisted admission counters, health ledgers or mandatory issue schemas.
