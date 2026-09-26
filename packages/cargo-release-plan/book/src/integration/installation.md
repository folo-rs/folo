# Install and inspect

Adoption starts with a known tool interface and a repository whose release history
is understood. You do not need to clone the tool's source repository.

## Select a published version

This walkthrough and its matching copied skill use the `0.4.1` tool interface.
Choose a tested published action revision that pins that interface. Check the selected
[action release](https://github.com/folo-rs/cargo-release-plan-action) and the
[crate's published versions](https://crates.io/crates/cargo-release-plan).
Confirm that the exact package and its promised native archives are published
before adoption. Source-mode tests are not evidence of published availability,
and a missing release is not a reason to substitute an older incompatible tool.

The command examples use PowerShell 7.4 or later for native-command error
handling. `Join-Path` keeps filesystem arguments native on Windows, Linux and
macOS. Use one installation method:

```powershell
$CrpVersion = "0.4.1"
cargo binstall "cargo-release-plan@$CrpVersion"
```

Or install the published source with its lockfile:

```powershell
$CrpVersion = "0.4.1"
cargo install cargo-release-plan --version "=$CrpVersion" --locked
```

Binstall uses a prebuilt executable where available and can fall back to source.
A successful fallback is useful installation behavior, but is not proof that a
promised release archive exists.

Verify the executable you will actually invoke:

```powershell
cargo release-plan --version
cargo release-plan --help
```

`--version` identifies the application, not the packages in your workspace. It
works without a Cargo workspace or Git repository.

The matching skill uses report/plan schema `4` and semantic-decision schema `1`.
Do not assume an arbitrary newer tool preserves those interfaces; update the
tool, action, documentation and copied skill deliberately.

## Separate the toolchains

Installing the application from source requires a compiler supported by that
application release. Your consumer repository may use a different compiler.
Select the installation toolchain explicitly when a local `rust-toolchain.toml`
would otherwise select an older one.

Registry upload operations require **Cargo 1.95 or later** for the workspace
publication behavior used by the tool. This runtime requirement is distinct
from the compiler needed to build the application. Source verification and tagged
binary builds must also satisfy the source's own toolchain and native build
requirements.

The published action pins the application and external checker versions and
prepares their installation separately from consumer builds. Binary sources
continue to supply their own toolchain, Cargo configuration and lockfile.

## Inspect your workspace before editing it

Run from the Cargo workspace root with Git and Cargo available:

```powershell
git status --short
cargo metadata --no-deps --format-version 1
cargo release-plan --version
```

Establish:

- Which branch actually releases, and whether full first-parent history exists.
- Which tracked members are publishable, private implementation packages or
  nonpublishable helpers.
- Which exact dependencies intentionally form version groups.
- Which packages contain an installable binary and what each executable is named.
- Whether the committed lockfile and toolchain describe a reproducible source
  build.
- Whether current registry versions and existing tags correspond to known
  publication source.

The final item needs an [existing-repository adoption audit](../operations/first-publication.md#adopting-an-already-published-workspace),
not just matching version strings.

Use an ignored, workspace-local directory for evidence, such as
`.release-plan-work`. Keep each assessment's inputs and outputs separate. Do not
commit generated plans as an alternative source of version truth.

Continue with [repository configuration](repository.md).
