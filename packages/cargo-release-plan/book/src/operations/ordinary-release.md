# An ordinary release

Once the packages, Trusted Publishers and shared workflows are configured, an
ordinary release is part of the PR process. For a new package, complete
[first publication](first-publication.md) before its first merge.

## Make the change and invoke the skill

Develop the change with its tests and public documentation, then ask the agent:

> Run increment-versions for this PR and update its Version/release plan.

The [installed skill](../integration/local-planning.md) handles compatibility
assessment, version selection, dependent-package changes and application of the
plan. You do not need to choose tool commands, edit generated planning files or
assemble the release table yourself.

## Review the proposed release

Review the source changes and the skill's generated **Version/release plan**
together. For a compatible operation added to `widget`, the table might contain:

| Package or group | Previous version | Proposed version | Reason |
| --- | --- | --- | --- |
| `widget`, `widget_impl`, `widget-fixtures` | `1.4.0` | `1.5.0` | Compatible public operation; implementation and fixture helper align with the group. The helper is not published. |
| `widget-cli` | `2.0.0` | `2.0.1` | Dependency update without a stronger CLI change; its existing pending patch increment is sufficient. |

The review question is whether those reasons accurately describe the change.
For example, removing or changing an existing operation's promised behavior may
require a breaking release instead. A dependent package's movement should have a
clear explanation rather than an invented user-facing feature.

If the table's reasoning does not match the source, ask the agent to correct the
assessment and rerun the skill. If source or release history changes during
review, have it refresh the plan rather than editing version numbers or generated
artifacts by hand.

## Merge under normal repository policy

Review and the required checks approve the source and version changes together.
Invoking the skill is not itself merge authorization.

After an authorized merge, the shared workflow publishes the requested packages,
tags, releases and native archives automatically. Successful publication needs no
separate manual delivery audit.

If something goes wrong, the release workflow fails and its reporter creates or
updates a failure issue. Follow [failure recovery](recovery.md) for the safe retry
or repair action instead of starting another version increment by default.
