# Plan versions with an agent

The supported version-planning interface is the **`increment-versions` skill**.
The CLI supplies the operations that the skill executes; manually assembling a
different command sequence is not a supported planning workflow.

The skill prepares a PR containing the source changes, version changes and an
explanation of the complete release. Review approves that contribution together.
There is no separate prompt to approve each version choice.

## Install the complete skill

Copy the complete
[skill directory](https://github.com/folo-rs/folo/tree/main/.github/skills/increment-versions)
from a known toolkit revision into `.github/skills/increment-versions` in your
repository. Include its command reference, decision guide and license.

Record the source revision in your repository's adoption notes. The skill checks
the schemas reported by the installed tool before editing source; it does not
require matching package version numbers. On a mismatch, update the skill from
the canonical repository. When developing that canonical skill, report the
mismatch rather than overwriting it.

The agent needs Git, the selected Cargo/Rust toolchain, `cargo-release-plan` and
`cargo-semver-checks`. Tool installation and upgrades follow your authorization
policy. A private configured repository also needs authenticated history access.

## Tell the agent what to assess

For configured publication, record the release branch and required checks in the
repository's agent instructions and provide `.cargo/release_plan.toml`. Then ask,
for example:

> Run increment-versions for this PR, assess the complete release impact, apply
> the resulting plan and update the PR's Version/release plan.

For a stacked PR, identify the actual parent branch or PR. The skill must preserve
the distinction between actual release history and the parent's anticipated final
release, rather than silently using the release branch as the child's target.

Tracked files may have staged or unstaged edits. The agent does not need a clean
checkout or a prior commit to assess them. Newly created intended release inputs
must be tracked before the assessment.

For one-off planning without publishing configuration, explicitly request
[standalone mode](../advanced/standalone.md).

## What the skill produces

The skill reads the complete release report and API comparisons, chooses each
package's semantic impact, previews group and dependency effects, and applies the
resolved plan. It retains reports and supporting files outside published source
so it can verify that the applied result is the one it assessed.

The PR explains:

- Which package versions change and why.
- Group alignment and dependent-package changes, not only directly edited code.
- Existing pending increments retained because they are sufficient.
- Any unavailable API comparison or other unresolved prerequisite.

The API checker establishes only a minimum impact from the contracts it can see.
The agent also considers behavior, CLI arguments, data formats and feature
availability. Human review remains necessary.

## Keep the result tied to the reviewed source

If source, release history or the parent changes, have the skill reassess. Do not
edit generated plans or reuse a report from a different checkout as permission
to apply changes. Repeating the skill retains an adequate pending increment; it
does not increase versions solely because it ran again.

Configured mode includes publication-readiness checks and the
[first-publication handoff](../operations/first-publication.md) when required.
Standalone mode stops at its version-increment PR. Neither invocation grants
merge or publication authority by itself.
