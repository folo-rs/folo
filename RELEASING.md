# Releasing Folo packages

Folo releases reviewed version changes on merge to `main`. Use the self-contained
`increment-versions` skill and keep the PR's
[Version/release plan](docs/git-workflow.md#versionrelease-plan-section) current.
Human review of the complete contribution is the approval step.

The public [release-process book](https://folo-rs.github.io/folo/cargo-release-plan/)
explains concepts, local planning, workflow integration and operation.
[Folo versioning](docs/release-versioning.md) and
[Folo automation](docs/release-automation.md) select this repository's policy.

## Ordinary releases

Validate the affected contribution under [the build policy](docs/build-and-tooling.md).
After authorized merge, `.github/workflows/release.yml` publishes and reconciles
the declared versions. Use the book's
[ordinary release walkthrough](https://folo-rs.github.io/folo/cargo-release-plan/operations/ordinary-release.html)
and [verification procedure](https://folo-rs.github.io/folo/cargo-release-plan/operations/verification.html).
Do not run `just gh-release` manually; it is a CI-only publishing entry point.

## First publish of a new crate

Follow the [first-publication guide](https://folo-rs.github.io/folo/cargo-release-plan/operations/first-publication.html):
maintainer bootstrap happens from the feature branch before its first merge,
in dependency order. Configure Trusted Publishing for owner `folo-rs`, repository
`folo`, workflow `release.yml`. The first merge must carry a higher version for the
second publication, which is the first automated release.

The generic skill reports this handoff but does not perform it. Its
`check-published` workspace scan is advisory; its resolved-plan check fails closed
for missing or unavailable registry prerequisites. A package without a Git anchor
may already have a manually published bootstrap version.

## Recovery and emergency operation

Use [the recovery guide](https://folo-rs.github.io/folo/cargo-release-plan/operations/recovery.html)
and the original failure issue. Manual tag recovery uses the exact source commit
recorded there, followed by retry of the original failed workflow. Existing tags
never move. Emergency registry publication requires explicit maintainer authority;
it is not an action performed by the version-planning skill.

## Required GitHub configuration

`main` is protected and uses the merge queue. The ruleset requires only the check
named `required-checks`; individual conditionally selected matrix jobs are not
ruleset requirements.

Each publishable crate has its Trusted Publisher registration for the caller
`release.yml`, with any selected protected environment matching that registration.
The action's separate required installation gate verifies actual published tool
versions and archives before an action release.
