# From changes to a version plan

The skill must answer two different questions: **what changed**, and **what
versions those changes require**. A diff can answer the first question, but it
cannot determine whether a behavioral change breaks a promise to users.

## Facts first, then semantic impact

A **release report** lists each package's changed released content, declared
version and workspace dependencies. File patches explain source changes;
structured entries explain inherited manifest values and binary dependency changes.
Together with API comparison results, these are the facts used to choose versions.

A **semantic impact** describes what a package's change means to its users:
`breaking`, `nonbreaking` or `patch`. The skill chooses it from the report,
API comparisons and the package's behavioral contracts. Interpreting those facts
and choosing the impacts is the **release assessment**.

For example, removing a public method is a breaking impact. Fixing a computation
without withdrawing a promise is usually a patch impact. A Rust API checker can
detect the removed method, but deciding whether the computation is a correction
still requires understanding the promised behavior.

## Preview makes the complete release visible

The skill turns the selected impacts into a **version proposal**. A proposal
starts with the changed packages, but a workspace release can affect more than
those packages:

- Members of a version group must move together.
- Dependencies must name the new versions.
- A binary can need a release because its locked dependencies changed.

**Preview** computes these effects and produces a **resolved plan**: the complete
package versions and exact manifest/lockfile edits to apply. Group expansion is
part of that operation, not a separate user step.

Consider a compatible API addition in `widget`:

| Package | Starting version | Result | Why |
| --- | --- | --- | --- |
| `widget` | `1.4.0` | `1.5.0` | A compatible API addition. |
| `widget_impl` | `1.4.0` | `1.5.0` | It shares `widget`'s version group. |
| `widget-cli` | `2.0.0` | `2.0.1` | Its dependency and locked binary inputs change. |

The proposal begins with the API decision. Preview makes the complete set visible
so the skill can assess the dependent changes too. A required dependent release
is a minimum obligation, not a claim that every such change is harmless.

## Apply the result that was reviewed

Suppose preview records `widget` at `1.5.0` and a particular dependency resolution.
Application must install those exact edits, not run resolution again and silently
choose a newer dependency. The resolved plan therefore also records the original
inputs that those edits may replace.

If a manifest or source file changes after preview, the skill prepares a fresh
plan. Editing the old generated plan by hand would hide which inputs were assessed.
Applying a valid plan twice is a no-op the second time; a partially edited tree is
not automatically treated as that completed result.

The same source binding matters for API checks. Comparisons before application
use the preview's retained workspace, where the proposed versions and lockfile
are already present. Comparing an unrelated checkout would say nothing about the
release described by the plan.

## Planning files are not publishing instructions

The skill preserves its reports, comparisons and plan while preparing the PR.
After merge, the publisher reads the approved versions from the merged source;
it does not need the agent's local working files.

Publication has a separate request that survives retries. That is the purpose of
the [publication manifest](publication.md).
