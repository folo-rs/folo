# Guide to releasing a new version

Publishing to crates.io and shipping `cargo-binstall` prebuilt binaries is automated
by `.github/workflows/release.yml` on every push to `main`. Pull requests that change
released content carry the version increments; merge publishes those versions.
The `increment-versions` skill decides and applies the plan without a separate
approval request. Human review of the complete PR, including its current
**Version/release plan** section, is the approval step; see
[docs/git-workflow.md](docs/git-workflow.md#versionrelease-plan-section).
See [docs/release-versioning.md](docs/release-versioning.md) for how versions are
decided and [docs/release-automation.md](docs/release-automation.md) for the publish
design.

`main` is behind a merge queue whose only required status check is `required-checks`.
See [Required GitHub configuration](#required-github-configuration) below.

1. Validate everything via `just validate` on Windows (will automatically invoke Linux validation).
1. If you feel like it, also perform extra validation via `just validate-extra`.
1. On merge to `main`, `release.yml` publishes any version crates.io does not yet have
   (via crates.io Trusted Publishing — no stored token) and uploads prebuilt binaries
   for the binary crates. If anything fails it opens a `ci-failure` issue for that run.

## First publish of a new crate

crates.io does not allow Trusted Publishing for a crate that has never been published,
so a brand-new crate's first version must be published manually **before its first
merge**. The maintainer publishes from the feature branch containing the new crate,
not from `main`, in dependency order:

1. Publish the initial bootstrap version with `cargo publish -p <crate>`.
1. Configure Trusted Publishing for the crate on crates.io (owner `folo-rs`, repo
   `folo`, workflow `release.yml`).
1. Prepare a version strictly higher than the bootstrap version for the first merge,
   including any version-group alignment and dependency requirement updates. Refresh
   the pull request's version/release plan against that intended automated release.
1. Merge the pull request. `release.yml` performs the crate's **second publication**,
   its first automated release. For a binary crate, the workflow also creates its
   GitHub release and uploads prebuilt binaries.

Do not merge the bootstrap version unchanged or rerun a `main` release workflow to
publish source that exists only on the feature branch. If the automated release needs
a retry after merge, rerun `release.yml` for the merged version. Package tags and
binary assets are reconciled against verified release-equivalent main snapshots;
existing tags are never moved.

The `increment-versions` skill runs `just check-never-published` as an early,
workspace-wide advisory. Before applying a resolved plan,
`just check-increment-published` fails unless every **publishable** package the
plan reaches has already reached crates.io. Version-alignment targets with
publication disabled do not require a first-publication handoff. The gate cannot
verify Trusted Publisher configuration, so complete that setup explicitly before
retrying the increment. The automated release follows the first merge.
The skill only reports this pre-merge maintainer handoff; it does not perform a manual
first publication or an emergency publish. A package can lack a Git release anchor
while its bootstrap version already exists on crates.io.

The skill prepares offline dependency resolution and previews prospective version
and requirement rewrites before application. The resolved plan includes the complete
release set and resolved lockfile. Application uses those captured files without
a late update; changed inputs require fresh preparation. Library-only lockfile
changes do not require releases, but their version rewrites still require a
consistent lockfile.

## Emergency manual publish

If the CI publish path is broken, publish by hand with `cargo publish -p <crate>` (in
dependency order). For a binary crate, re-run `release.yml` (or push a version bump)
afterwards so the prebuilt binaries are produced.

## Required GitHub configuration

Branch protection, the merge queue, and the required-status-check ruleset are GitHub
settings rather than files in this repository, so they are configured once by a
repository admin and are prerequisites of the process above:

* `main` is protected.
* The merge queue is enabled on `main`.
* The ruleset requires only the status check named `required-checks`.
* Individual Standard validation matrix job names are not required — a skipped leg never
  posts a check and would block the queue forever.

`cargo-release-plan` also needs a one-time first `cargo publish` (and Trusted
Publishing configured afterwards) before later versions can go through `release.yml`.
