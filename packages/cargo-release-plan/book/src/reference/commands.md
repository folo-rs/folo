# Command tasks and prerequisites

Use `cargo release-plan --version` to identify the executable and
`cargo release-plan --help` for its command interface. Match that executable to
the documentation and skill revision before making changes.

Angle-bracket values below describe arguments to supply, not literal shell
input. The [integration walkthrough](../integration/local-planning.md) provides
concrete PowerShell command sequences.

## Release context and identity

```text
cargo release-plan release-context [--manifest-path <path>] [--config <path>]
    [--base <rev>] [--verbose]
cargo release-plan check-publishing-identity [--verbose]
```

`release-context` loads committed publication configuration and prints
`repository`, `release_branch`, `release_base`, `head`, `workspace_manifest`,
`config_path` and `concurrency_group`. It fetches the configured release branch
unless `--base` supplies the tested baseline explicitly. It accepts dirty source
and does not prepare a publication manifest.

`check-publishing-identity` requires GitHub Actions OIDC context but no workspace.
It exchanges and revokes a short-lived crates.io credential without uploading.
Success verifies the caller identity path, not each package's publisher grant.
Do not pass workspace configuration to this command.

## Assessment

```text
cargo release-plan report --out-dir <dir> [--base <rev>] [--manifest-path <path>] [--verbose]
cargo release-plan check [--base <rev>] [--manifest-path <path>] [--config <path>]
    [--format text|github] [--verify-packaging] [--verbose]
```

`report` writes `report.json` and patches for released file changes. Enumerate
the report's statuses, not the patch directory, to find every affected package.

`check` fails on missing increments, inconsistent groups, malformed/stale
workspace requirements and required public-dependency breaking propagation.
With `--config`, it also validates publication inputs offline.

Ordinary assessment uses Git and `cargo metadata --no-deps`: it does not fetch,
contact crates.io, resolve dependencies or compile.

`--verify-packaging` is an explicit audit against `cargo package --list`.
Divergences are warnings rather than changes to the verdict. This probe performs
Cargo package preparation and resolution, and allows dirty trees; investigate
clean-tree divergence rather than treating it as proof that the ordinary check
performed the same work.

## Prepare, choose and preview

```text
cargo release-plan prepare --output <dir> [--base <rev>] [--manifest-path <path>] [--verbose]
cargo release-plan analysis-order --report <file-or-dir> [--verbose]
cargo release-plan semver-targets --report <file-or-dir> [--verbose]
cargo release-plan propose --report <file-or-dir> --decisions <decisions.json>
    --out <plan.json> [--verbose]
cargo release-plan preview --prepared <prepared.json> --plan <plan.json> --output <dir>
    [--manifest-path <path>] [--verbose]
```

`prepare` performs the intended `cargo update --offline --workspace` resolution
and can modify the lockfile. `preview` resolves prospective changes in its
retained workspace before capturing the final applicable result. Neither
requests blanket third-party upgrades.

`analysis-order`, `semver-targets` and `propose` operate only on supplied
artifacts. A report argument can be a JSON file or a directory containing
`report.json`. They invoke neither Git nor Cargo. The first two print JSON to
stdout; `propose` writes its plan to `--out` and prints a readable summary.

## Compatibility and publication preflight

```text
cargo release-plan check-compatibility --output <new-directory> [--manifest-path <path>]
    [--prepared <prepared.json> | --plan <resolved-plan.json> | --base <rev>]
    [--deny-findings] [--verbose]
cargo release-plan check-published [--plan <resolved-plan.json>]
    [--manifest-path <path>] [--verbose]
```

`check-compatibility` collects external API evidence without choosing semantic
levels. Select exactly one source mode:

- `--prepared` checks the captured original prepared inputs.
- `--plan` checks a resolved preview's retained workspace.
- Without either, a fresh read-only report is acquired against `--base` or its
  ordinary default.

It regenerates a report bound to the selected source and verifies captured inputs
before and after checking. It never accepts detached `--report` evidence.
The existing report/plan schema remains `4`.

The new output directory contains `compatibility.json`, `semver-checks.log` and
the read-only report. Evidence records the checker identity, exact published
comparison versions, completed comparisons and semantic floors. An
identical-source canary verifies the checker. An empty contract selection
requires neither checker execution nor registry access.

Operational failures do not become compatibility passes. Completed findings are
planning evidence by default; `--deny-findings` makes an insufficient increment
fail the command as well. Read `completed`, `compared` and `required_level`,
not merely the exit code.

`check-published --plan` validates the resolved target set and fails on a
never-published or indeterminate publishable target. Alignment-only helpers are
excluded. Without a plan, workspace-wide missing/unknown-package discovery is
advisory. Neither mode uploads a first version or verifies Trusted Publisher
administration.

## Inspect and apply

```text
cargo release-plan expand --plan <plan.json> --out <expanded.json>
    [--manifest-path <path>] [--preserve-input] [--verbose]
cargo release-plan inspect-plan --plan <expanded.json> [--require-resolved]
    [--manifest-path <path>] [--verbose]
cargo release-plan verify-preview --plan <plan.json> --manifest-path <prospective-manifest>
    [--verbose]
cargo release-plan apply --plan <plan.json> [--dry-run] [--manifest-path <path>] [--verbose]
```

`expand` performs structural expansion only. `--preserve-input` protects the
proposal from input/output aliases and stages the result before replacing the
destination.

`inspect-plan --require-resolved` validates captured inputs and the retained
compatibility workspace, then prints publication target names and its manifest
path. Alignment-only helpers are not publication targets.

`verify-preview` checks that external analysis left the prospective workspace
unchanged. It does not resolve or compile.

Resolved `apply` installs captured manifests and the lockfile without resolving
again. Use `--dry-run` for read-only validation. Applying an unexpanded proposal
is a separate low-level manifest-only operation, not the complete release
workflow.

## Prepare and publish exact versions

```text
cargo release-plan prepare-publish --source <full-commit-SHA> --output <manifest.json>
    [--manifest-path <path>] [--config <path>] [--verbose]
cargo release-plan publish registry --publication <manifest.json> --output <new-outcome.json>
    [--manifest-path <original-source-Cargo.toml>] [--dry-run] [--verbose]
cargo release-plan publish github --publication <manifest.json> --output <new-outcome.json>
    --batches <new-directory> [--manifest-path <original-source-Cargo.toml>]
    [--dry-run] [--verbose]
cargo release-plan publish binaries --publication <manifest.json> --batch <batch.json>
    --output <new-outcome.json> --artifacts <new-directory>
    [--manifest-path <original-source-Cargo.toml>] [--no-upload]
```

Preparation requires clean source with HEAD matching the full supplied SHA,
tracked configuration and Cargo inputs, a consistent lockfile, and first-parent
membership in the configured release branch. It fetches that branch but makes
no remote publication writes.

Registry publication requires the original captured source. It queries exact
availability and delegates verification and ordered uploads to Cargo. Automatic
uploads require Cargo 1.95 or later and the calling workflow's crates.io Trusted
Publisher registration with GitHub OIDC.

Registry `--dry-run` reads availability without exchanging credentials or
uploading. GitHub `--dry-run` observes tags, releases and assets without writes.
Neither dry run is a complete publication receipt.

`publish github` verifies registry availability for the complete manifest before
reconciling package tags and binary releases. It writes an outcome and frozen
native batches with relative routing paths and stable `batch_id` values.
Missing-tag failures retain original-source recovery instructions and do not
suppress independent valid work.

`publish binaries` consumes one frozen batch and its parent manifest. It
rechecks tag identity and current assets, builds each executable from the
recorded tag commit, and completes missing ZIP/checksum pairs. `--no-upload`
stages that frozen work without querying or writing GitHub, retaining source
and archive verification.

Every attempt needs new outcome and batch/artifact destinations. Name phase
receipts `outcome.json` inside separate artifact subdirectories when handing
them to the final reporter. The
[publication walkthrough](../integration/publication.md) shows the complete
sequence without caller-authored batch JSON.

## Final publication reporting

```text
cargo release-plan publish report --repository <owner/repo>
    --outcomes <download-directory> --jobs <jobs.json> --output <new-report.md>
    [--publication <manifest.json>] [--no-issue]
```

Run this in the original GitHub workflow context after downloading phase
artifacts. It reads retained `outcome.json` files and the fixed current job
results, then writes Markdown and creates or updates a run-qualified failure
issue when publication is incomplete.

The manifest can be omitted when unavailable; the report still runs but cannot
claim complete delivery. Incomplete publication exits nonzero even when Markdown
and issue creation succeed. `--no-issue` suppresses GitHub writes only.

Binary receipts must match both publication and batch identity and be at least
as new as the GitHub reconciliation that requested the work. A stable batch
identity does not let an older success override newly observed missing assets.

## Paths and output

`--manifest-path` defaults to `Cargo.toml` in the current directory. For planning,
`--base` defaults to the branch advertised by `origin`, then `origin/main`.
Automation should supply a frozen baseline explicitly. `release-context` is the
explicit configuration-aware acquisition operation; unlike ordinary assessment,
it fetches the configured branch when no tested `--base` is supplied.

Ordinary artifact arguments resolve from the invocation directory. Publication
`--config` resolves from the selected workspace. Paths recorded in transported
publication artifacts are repository-relative, not host-absolute paths.

Requested machine output stays separate from diagnostics. `--verbose` writes
explanatory selection and validation information to stderr.

## Runtime boundaries

| Work | Prerequisites and effects |
| --- | --- |
| Identity/help | Installed executable; no workspace needed. |
| Release context | Configuration, Git and release-history access unless the caller supplies a tested baseline; no clean-source requirement. |
| Publishing identity probe | GitHub Actions OIDC identity and crates.io exchange/revocation access; no Cargo workspace or upload. |
| Artifact-only planning | Supported report/decision artifacts; no checkout or network. |
| Ordinary assessment | Git history, work tree and Cargo metadata; offline and no resolution. |
| Preparation/preview | Cargo and available offline resolution inputs; can change the relevant lockfile. |
| External API checking | Compatible checker/compiler and comparison inputs; can compile and retrieve registry sources. |
| Publication preflight | Registry reads; plan-scoped checks fail closed, workspace discovery is advisory. |
| Publication preparation | Clean merged source, Git/Cargo, configuration and release-branch access; private repositories require GitHub authentication. |
| Live registry work | crates.io access, supported Cargo, registered caller OIDC identity and package verification prerequisites. |
| Native binaries | Native runner, source toolchain/build dependencies and archive tools; upload authority is separate from compilation. |
| Final report | Retained outcomes, current job results and GitHub run context; issue writes unless `--no-issue`. |

Do not treat the compiler used to install the controller, Cargo's publication
runtime floor and a tagged source's toolchain as one interchangeable requirement.
