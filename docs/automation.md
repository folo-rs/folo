# Automation guidelines

Repository automation should be understandable from ordinary GitHub records and
inexpensive to operate as repository history grows. Use the smallest amount of
metadata and discovery needed for the action being taken.

For implementation language, script structure and validation, follow
[build and tooling](build-and-tooling.md#automation-language-and-boundaries).
Domain-specific behavior belongs in the owning design documentation, such as
[scheduled validation](scheduled-validation.md).

## Avoid redundant labels

Do not create a label merely to identify an issue kind that its title already
identifies reliably. Prefer an existing fixed title prefix when it is sufficient,
and document that convention for both producers and consumers. Do not combine
title and label requirements that encode the same fact.

A new label needs a distinct purpose that existing issue state, titles,
assignees, discussion or linked PRs cannot adequately express. Explain that
purpose in the owning design. An independent human-action blocker or deliberate
enrollment of otherwise ordinary issues can justify a label; convenience for an
automation query alone does not.

Do not replace an unnecessary label with a hidden issue-body protocol or a
private registry. Human-readable conventions should remain sufficient to
understand the work and its next action.

## Keep discovery narrow

Default work discovery to open issues in the relevant repository. Narrow the
server-side query by the known issue kind before fetching content or discussion.
When the API cannot express an exact title-prefix match, use title search and
apply the exact prefix check to the returned metadata before further reads.
Exclude pull requests when discovering issues.

Do not enumerate the repository's entire issue inventory and then filter locally.
Do not routinely include closed issues, or broaden a failed or empty query to all
history as a precaution. Closed issues dominate a mature repository and usually
do not represent actionable work.

Historical reads need a concrete purpose and a narrow scope: for example, follow
a known issue by number to establish a repair's disposition, or search for a
specific previously diagnosed problem to investigate recurrence. A need to read
one closed issue does not justify scanning the closed backlog. Explain a recurring
historical query's necessity in the owning design rather than treating it as the
default discovery strategy.

Paginate the selected scope completely. Failed or truncated discovery is not an
empty queue, and a result limit must not silently discard work. Narrow the query
when possible or surface the limitation; do not add a broader fallback. Likewise,
do not hide old open work behind an arbitrary recent-date cutoff.

Before a state-dependent action, confirm the selected issue's current eligibility.
Search results can lag title edits and closure; refreshing a selected candidate
does not require rediscovering the repository.

## Make notifications useful and repeatable

A notification should explain an observed problem and the concrete corrective
action, not merely announce that automation skipped an issue. Avoid repeated
comments for the same condition.

When a workflow requires a one-time notification, use a stable, documented comment
marker to find the existing notification in that issue's discussion. Keep the
explanation human-readable; the marker only deduplicates delivery and is not an
issue classification, ownership claim or eligibility verdict. Do not create a
label to record that a comment was posted.

Read the relevant discussion completely before posting. Reconcile an ambiguous
write by reading back its effect rather than blindly repeating it. Continue
evaluating current eligibility on later visits so a prior notification does not
prevent work after the issue is corrected.
