# One-off version increments

Use the `increment-versions` skill in **standalone mode** when you want a
version-increment PR without adopting publishing integration. The user supplies
the planning inputs instead of `release_plan.toml`; no publishing configuration,
GitHub remote or workflow installation is required.

The scenario ends with the reviewed version-increment PR being merged under
the repository's normal authorization policy. Publishing after that merge is
outside standalone mode. The shared workflow remains the toolkit's canonical
supported publishing path.

## Provide inputs, not a publishing configuration

Tell the agent which Cargo workspace to assess, the local ref or commit delimiting
release history, and the parent target ref for a stacked PR.

For example:

> Run increment-versions in standalone mode for this workspace. Use my local
> `origin/main` ref as release history and no unmerged parent. Prepare the
> version-increment PR; do not configure or run publication.

Those ref names are your choices, not defaults inferred from a publishing setup.
A repository without remotes can use a local branch or commit. If you want a
remote-tracking ref refreshed first, say so.

The [agent prerequisites](local-planning.md#install-the-complete-skill) still apply.
Tracked files may have staged or unstaged changes; newly created intended release
inputs must be tracked before assessment.

## The same version model, without publication setup

The skill uses the same reports, semantic impacts, version groups, dependency
effects and captured-plan checks as configured planning. It supplies your selected
history and target through the existing tool inputs; it does not create a dummy
publishing configuration.

It does not require registry identities, Trusted Publisher bootstrap or binary
archive metadata merely to edit versions. API comparison remains separate
supporting information and can still read published crates. An unavailable
comparison is not proof of compatibility.

Standalone mode is explicit, not a fallback for bypassing a failed configured
check. Existing repository CI and merge-triggered automation remain in force;
choosing this planning mode does not disable them.

## Review and merge the version PR

Review the code and complete version explanation together, including dependent
packages and retained pending versions. If the source or selected refs move, have
the skill reassess rather than editing its generated files.

The skill does not grant merge permission. Once the version-increment PR is
merged with normal authorization, the standalone workflow is complete.
