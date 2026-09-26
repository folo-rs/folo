# Upgrade tools, actions and skills

The application, GitHub integration and copied skill have related interfaces but
different distribution lifecycles. Upgrade them as a tested combination rather
than assuming equal version numbers.

The walkthrough selects the `0.4.1` application interface and matching copied
skill, with report/plan schema `4` and semantic-decision schema `1`.
`ACTION_REVISION` in workflow examples must be replaced with the verified
immutable commit of a tested published action release selecting that interface.
Use the same commit for the root composite, check, release and identity-probe
workflows. The placeholder is not a claim that an action tag already exists.

## Keep an adoption record

Record:

- The immutable action revision.
- Its exact `cargo-release-plan` and external checker pins.
- The immutable source revision of the copied skill directory.
- The source revision of the book used for that combination.
- Consumer-owned external-type checker/nightly pins and supported API targets.

The action's release manifest is authoritative for its installed tools. A
published action revision does not accept a silently substituted application
version underneath it. Source mode is an explicit, different selection.

The website follows current documentation. To read an older combination, use
GitHub's tag or commit selector in the
[book source directory](https://github.com/folo-rs/folo/tree/main/packages/cargo-release-plan/book)
and read the Markdown at the recorded immutable revision. Use the same method
for `.github/skills/increment-versions`. Do not copy the latest skill while
retaining an older unsupported command interface.

## Upgrade checklist

1. Read the chosen release's command, configuration and artifact compatibility
   notes.
2. Confirm the exact application package and promised native archives exist.
3. Verify the installed executable with `cargo release-plan --version`, without
   relying on an old executable cache.
4. Exercise the selected action revision in read-only checks on a representative
   consumer workspace.
5. Copy the complete matching skill directory and record its revision.
6. Verify OIDC exchange/revocation with the identity probe when the caller or
   publishing environment changes, and confirm package-specific grants separately.
7. Regenerate stale local preparation/report/preview evidence rather than
   editing schema numbers.
8. Inspect retained publication runs before changing the tool used for retries.
9. Review and merge the coordinated configuration and pin changes.

A moving action major reference is convenient but can resolve to different
source on a later invocation. An immutable commit or release tag gives a stable
selection, including for full workflow reruns. The reusable workflow's internal
composite also uses that same immutable revision.

## Artifact compatibility

Reports and version plans have a schema lifecycle separate from publication
manifests and outcomes. Neither an action version nor a package version is an
artifact schema number.

Unsupported local-planning formats require new `prepare` and `preview` evidence.
Captured file content and input identity are not fields to repair by hand.

For an incomplete publication, prefer the original compatible tool and retained
manifest. If that combination cannot be restored, explicitly prepare new
evidence from the retained original source under the recovery procedure.
Do not replace original intent with current branch content or pretend that a
new manifest is an old outcome.

## Verify distribution, not just source

An action release must be usable with its exact published pins. Source-mode
tests do not establish that a crates.io package exists or that each supported
archive installs without fallback.

Test source installation and promised native binary installation separately.
Verify external checker installation independently; the release action does not
manufacture that external tool's archives. Update the external-type checker and
its rustdoc nightly together.

Adding a target requires published tool support, native runner provisioning,
archive verification and an assessed action/tool release. It is not a consumer
configuration shortcut around the supported target set.
