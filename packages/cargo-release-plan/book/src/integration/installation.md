# Install and inspect

Adoption starts with a known tool interface and a repository whose release history
is understood. You do not need to clone the tool's source repository.

## Select a published version

Choose a tested published action revision whose tool supports the schemas documented
in this book and copied skill. Check the selected
[action release](https://github.com/folo-rs/cargo-release-plan-action) and the
[crate's published versions](https://crates.io/crates/cargo-release-plan).
Confirm that the exact package and its promised native archives are published
before adoption. Source-mode tests are not evidence of published availability,
and a missing release is not a reason to substitute an older incompatible tool.

The command examples target the tested PowerShell 7.6 support baseline, independently
of the application's CLI portability. This is a support policy, not a claim that
every example uses a newly introduced language feature. Multi-step procedures explicitly
enable terminating native-command errors; the PowerShell version alone does not
enable that behavior. `Join-Path` keeps filesystem arguments native on Windows,
Linux and macOS. Use one installation method:

```powershell
$CrpVersion = "<published-version>"
cargo binstall "cargo-release-plan@$CrpVersion" --locked
```

Or install the published source with its lockfile:

```powershell
$CrpVersion = "<published-version>"
cargo install cargo-release-plan --version "=$CrpVersion" --locked
```

Replace `<published-version>` with the exact tool version selected by that action.
Binstall uses a prebuilt executable where available and can fall back to source.
A successful fallback is useful installation behavior, but is not proof that a
promised release archive exists.

Verify the executable you will actually invoke:

```powershell
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
cargo release-plan version
cargo release-plan --help
```

`version` prints the application's version and supported schema revisions as JSON,
without a Cargo workspace or Git repository. The short `--version` identity remains
available for installation probes.

The matching skill uses report/plan/prepared schema `6`, semantic-decision and
compatibility schema `2`, and release-context schema `2`. Exact skill/tool package
version synchronization is unnecessary; check those schemas. If unexpected CLI
errors suggest a mismatch, consider upgrading both tool and skill.

## Separate the toolchains

Installing the application from source requires a compiler supported by that
application release. Your consumer repository may use a different compiler.
Select the installation toolchain explicitly when a local `rust-toolchain.toml`
would otherwise select an older one.

Source installation also needs a native C/C++ build toolchain and CMake for the
application's dependencies. Installing a matching prebuilt binary avoids that
compilation requirement.

Registry upload operations require **Cargo 1.95 or later** for the workspace
publication behavior used by the tool. This runtime requirement is distinct
from the compiler needed to build the application. Source verification and tagged
binary builds must also satisfy the source's own toolchain and native build
requirements.

The published action pins the application and API compatibility checker versions and
prepares their installation separately from consumer builds. Binary sources
continue to supply their own toolchain, Cargo configuration and lockfile.

Native binary publication uses Git, Cargo, rustup and the GitHub CLI. ZIP creation
and SHA-256 hashing are included in the application; no separate archiver needs
installation on the runner or a direct CLI user's machine.

## Inspect your workspace before editing it

Run from the Cargo workspace root with Git and Cargo available:

```powershell
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
git status --short
cargo metadata --no-deps --format-version 1
cargo release-plan --version
```

Establish:

- Which branch actually releases, and whether full first-parent history exists.
- Which tracked members are publishable, private implementation packages or
  non-publishable packages.
- Which exact dependencies intentionally form version groups.
- Which packages contain an installable binary and what each executable is named.
- Whether the committed lockfile and toolchain describe a reproducible source
  build.
- Whether current registry versions and existing tags correspond to known
  publication source.

Confirming registry versions and tags correspond to known publication source needs an
[existing-repository adoption audit](../operations/first-publication.md#adopting-an-already-published-workspace),
not just matching version strings.

Use an ignored, workspace-local directory for evidence, such as
`.release-plan-work`. Keep each assessment's inputs and outputs separate. Do not
commit generated plans as an alternative source of version truth.

Continue with [repository configuration](repository.md).
