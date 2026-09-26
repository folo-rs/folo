# Custom jobs and source identity

Prefer the reusable workflows for the standard process. Use the root composite
when your repository needs a different job arrangement or a supported target on
a different native runner. Neither option changes release policy.

## Root composite

The root action lives at
[`folo-rs/cargo-release-plan-action`](https://github.com/folo-rs/cargo-release-plan-action).
It selects an operation with `command`; the operation determines its additional
inputs. Consult the exact selected revision's input contract.

The shared inputs are:

| Input | Meaning |
| --- | --- |
| `command` | Select the supported operation, not arbitrary shell or Cargo arguments. |
| `working-directory` | Select the consumer Cargo workspace. |
| `config` | Select publication configuration relative to that workspace. |
| `install-method` | Choose `binstall`, `install` or explicit source mode `path`. |
| `source-path` | Select the tool source checkout for `path` installation. |

The root operations include `version`, `version-readiness`, `check`,
`prepare-publish` and `publish-registry`. `version-readiness` is the narrow
offline gate; do not treat it as full compatibility and publication-input
validation.

This step illustrates executable identity checking. Replace `ACTION_REVISION`
with the same verified immutable commit selected from a tested published action
release for every caller:

```yaml
- uses: folo-rs/cargo-release-plan-action@ACTION_REVISION
  with:
    command: version
    install-method: binstall
```

The action version is independent of the application version printed by this
operation. A released revision pins exact tools; `binstall` permits source
fallback, `install` builds the exact published source with its lockfile, and
`path` builds the selected tool source. Path mode must not restore a released
executable cache in place of that source.

Consumers of released tools do not need a Folo checkout. Only deliberate tool
development/source-mode testing supplies one.

## Shared workflow inputs

The public reusable workflows are `.github/workflows/check.yml`,
`.github/workflows/release.yml` and `.github/workflows/identity-probe.yml`
in the action repository:

| Input | Default | Workflows |
| --- | --- | --- |
| `working-directory` | `.` | Check and release |
| `config` | `.cargo/release_plan.toml` | Check and release |
| `install-method` | `binstall` | All |
| `source-path` | `.` | All |
| `publishing-environment` | Empty | Release and identity probe |

The identity probe needs neither a Cargo workspace nor configuration. It tests
OIDC exchange/revocation in the calling workflow's identity. Use the registered
entry workflow filename and publishing environment, not another caller and an
assumption that the identity is equivalent.

The [check caller](../integration/github-checks.md) and
[publication callers](../integration/publication.md) show complete examples.
Do not add arbitrary per-field release-policy inputs; committed configuration
remains the policy source.

## Preserve source identities

| Identity | Role |
| --- | --- |
| Frozen release baseline | Select history for pre-merge version assessment. |
| Per-package anchor | Supply that package's comparison content and version. |
| Prepared source and prospective workspace | Bind semantic evidence and the exact plan application. |
| Publication source | Fix the merged source and declared versions to deliver. |
| Peeled tag commit | Fix the source actually used to build one binary release. |
| Action/controller revision | Fix the implementation executing the workflow. |

Do not substitute one for another. In particular, a source checkout's
`rust-toolchain.toml` must not accidentally select an incompatible compiler for
installing the controller. Conversely, installing a newer controller does not
authorize building a historical binary with today's source and lockfile.

Tagged builds run with the tag's source as their working directory, not merely
a different `--manifest-path` while inheriting another checkout's Cargo
configuration. Standard builds use release mode, locked dependencies, the
selected native target and default features.

## Own the orchestration, not another release model

A custom graph must preserve these boundaries:

- Prepare immutable intent once; pass it unchanged to later phases.
- Require registry completion and fresh prerequisite validation before GitHub
  writes.
- Admit native work only from valid manifest-linked batches with established
  tag identities.
- Continue independent valid releases after a per-release failure while keeping
  the aggregate run failed.
- Keep credentials phase-local and out of source-build subprocesses.
- Retain input artifacts and attempt-specific diagnostics on failure.
- Treat missing artifacts as errors and retry original intent.

Do not hand-maintain package lists, derive upload order from semantic assessment
batches or synthesize platform-batch JSON. The tool owns selection and artifact
validation; Cargo owns registry ordering.

Use the fixed optional `.github/actions/release-plan-setup/action.yml` only to
prepare native build requirements. It is not an arbitrary release-policy hook
and cannot modify captured source.

For nested workspaces, keep `working-directory` and `config` consistent with
their [workspace-relative rules](../reference/configuration.md). Give independent
release contexts distinct concurrency and artifact identities, while preserving
one context across retries.

`release-context` supplies the configured `repository`, `release_branch`,
immutable `release_base` and stable `concurrency_group`. With an explicit
`--base`, it uses the tested history boundary without fetching another baseline.
It does not replace publication preparation or require a clean checkout.

Use the exact [phase commands](../reference/commands.md#prepare-and-publish-exact-versions)
for custom jobs. Download outcomes into distinct artifact subdirectories named
per run and attempt, preserving `outcome.json` and batch linkage. The final
reporter takes the fixed `prepare`, `registry`, `github` and `binaries` job results,
regardless of the display names in your graph.

## Troubleshoot at the boundary that failed

Use `--verbose` to see selection inputs and reasons without mixing diagnostics
into machine-readable output. Identify whether a failure occurred in offline
assessment, explicit resolution, external compilation, source validation,
registry observation or GitHub publication.

Do not fix an offline classification issue by adding registry lookups, a
compatibility execution failure by suppressing its result, or an artifact
mismatch by overwriting the artifact. Resolve the relevant input or tool
combination and regenerate evidence where required.
