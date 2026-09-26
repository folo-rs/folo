# Implementation package bootstrap

The private implementation packages require maintainer bootstrap before their first
merge. They are published only to make the command-line application installable;
none provides a supported Rust library API.

| Packages | Manual bootstrap | Intended first automated version |
| --- | --- | --- |
| `crp_diag`, `crp_workspace`, `crp_versioning`, `crp_native`, `crp_publication` | `0.4.0` | `0.4.1` |

Use the reviewed extraction source identified by the pull request. Publish the
bootstrap in dependency order: diagnostics, workspace, versioning and native,
then publication. The executable retains its pending `0.4.1`; it is not a
dependency of any bootstrap package. Exact component requirements name the
bootstrap versions until the higher group plan is applied.

Follow the [first-publication procedure](../../../RELEASING.md#first-publish-of-a-new-crate)
using explicitly authorized maintainer authentication. Configure each package's
Trusted Publisher for `folo-rs/folo` and the registered `release.yml` caller,
including its configured environment where applicable. Registry existence alone
does not establish that registration.

After setup, refresh the actual release-branch baseline and regenerate the
source-bound version plan. Require the plan-scoped registry preflight to pass,
apply the captured higher version unchanged, and verify group alignment,
dependency requirements and lockfiles before merge. Do not publish the first
automated version manually or merge the bootstrap versions unchanged.

Until that handoff is complete, the expanded plan is review evidence, not an
applied release. Source installation and passing native tests do not substitute
for published package/archive installation or authorize publication. The
coordinator owns action source pins and the existing rollout gates.
