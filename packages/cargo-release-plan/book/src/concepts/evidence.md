# Evidence and resolved plans

Planning separates evidence, judgment and application so that a dependency
refresh cannot quietly change the release after its versions have been chosen.

| Artifact or stage | Purpose |
| --- | --- |
| Prepared workspace and `prepared.json` | Perform the intended offline workspace resolution and capture the resulting inputs. |
| `report.json` and `diffs` | Explain changed released content, package status and workspace relationships. |
| Decisions document | Record the author's semantic levels. |
| Proposed plan | Translate those decisions into version choices and required propagation. |
| Preview | Resolve prospective versions and requirements in a retained disposable workspace. |
| Resolved expanded `plan.json` | Name every version target and capture its final versions, file edits and original input identity. |
| Post-application report | Verify the result without overwriting the evidence used to choose it. |

## Read the whole assessment

`report.json` is the complete verdict. Patches contain file changes, but inherited
workspace values and binary dependency identities are reported as structured
change entries, not invented file diffs. A package can need an increment without
having a patch.

`analysis-order` supplies dependency-first assessment batches. Mutually dependent
packages appear together; sharing a version group alone does not make a cycle.
`semver-targets` selects public library contracts for comparison. These
artifact-only operations do not query a checkout or a registry.

## A resolved example

Suppose `widget` gains a compatible operation implemented by `widget_impl`.
The helper stays in their version group, and `widget-cli` already carries a
sufficient pending patch increment. The following is **conceptual shorthand**,
not JSON to submit to the CLI:

```text
prepared evidence:
  widget:          new public operation, anchor 1.4.0
  widget_impl:     supporting implementation, anchor 1.4.0
  widget-fixtures: alignment-only, declared 1.4.0
  widget-cli:      pending 2.0.1, anchor 2.0.0

semantic decisions:
  widget: nonbreaking
  widget_impl: patch
  widget-cli: patch after assessing the prospective dependency change

resolved plan:
  widget:          1.5.0
  widget_impl:     1.5.0
  widget-fixtures: 1.5.0, not published
  widget-cli:      2.0.1, existing increment retained
  captured edits: manifests, dependent requirements, resolved Cargo.lock
```

Preview can discover that a binary's locked dependencies change only after the
proposed versions and requirements are resolved. Assess that new evidence before
application. A mechanically required release is a minimum obligation, not proof
that the effect is semantically a patch.

## Why expansion alone is insufficient

`expand` turns a proposal into explicit package/version entries without resolving
dependencies. It is useful for inspecting group membership, but does not capture
the lockfile effects needed for a complete release.

`preview` supplies the complete resolved artifact and retains the prospective
workspace for compatibility checks. `check-compatibility --plan` selects that
workspace, regenerates a read-only report and verifies the captured source
before and after comparison. `--prepared` instead checks the original prepared
inputs; without either selector, it acquires fresh evidence against the chosen
baseline.

A detached report does not establish that a checkout is still current, so there
is no `check-compatibility --report` mode. The report/plan schema remains `4`.
For additional external analysis, use the recorded
`resolved.evidence_manifest_path` and follow it with `verify-preview`.

The expanded target set includes nonpublishable alignment members. Dependents
whose requirements are rewritten without receiving another version are reflected
in the captured edits, not added as fictitious version movements.

## Apply captured state, not a new interpretation

Resolved application validates its original inputs and installs the captured
manifest and lockfile contents. It does not run resolution or quietly add
targets. `--dry-run` performs the read-only validation first.

Keep `prepared.json` and the resolved plan intact. They are tool-owned evidence,
not templates for hand editing. Source, baseline, membership or semantic changes
require fresh evidence and preview.

Applying the same artifact to its fully applied state is a no-op. A partially
changed tree is different: inspect it before recovering. Application validates
before writing, but filesystem failures can still interrupt writes.

After merge, publication does **not** consume the local planning files. It reads
the approved versions from clean merged source and captures a separate
[publication manifest](publication.md).
