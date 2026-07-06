# Guide to releasing a new version

Publishing to crates.io and shipping `cargo-binstall` prebuilt binaries is automated
by `.github/workflows/release.yml` on every push to `main`. Bumping versions is the
only manual step. See [docs/release-automation.md](docs/release-automation.md) for the
full design.

1. Validate everything via `just validate` on Windows (will automatically invoke Linux validation).
1. If you feel like it, also perform extra validation via `just validate-extra`.
1. Execute `just prepare-release` on the `main` branch to increment version numbers and update changelogs.
    * `just prepare-release` first verifies that `cargo semver-checks` can actually run
      (it aborts if the installed tool is too old for the current toolchain's rustdoc
      format); release-plz would otherwise silently treat a broken semver check as "no
      breaking changes".
    * `just prepare-release` also warns if a crate has never been published — such a
      crate's first release must be done manually (see "First publish" below).
    * Verify pending changes manually and adjust as necessary.
    * Commit as "chore: prepare for release" when satisfied with the changes.
    * `git push`
1. On push to `main`, `release.yml` publishes the bumped crates to crates.io (via
   crates.io Trusted Publishing — no stored token) and uploads prebuilt binaries for
   the binary crates. If anything fails it opens a `ci-failure` issue for that run.

## First publish of a new crate

crates.io does not allow Trusted Publishing for a crate that has never been published,
so a brand-new crate's first version must be published manually:

1. `cargo publish -p <crate>` (with a crates.io token login).
1. Configure Trusted Publishing for the crate on crates.io (owner `folo-rs`, repo
   `folo`, workflow `release.yml`).
1. Subsequent releases then go through `release.yml` automatically.

## Emergency manual publish

If the CI publish path is broken, publish by hand with `cargo publish -p <crate>` (in
dependency order). For a binary crate, re-run `release.yml` (or push a version bump)
afterwards so the prebuilt binaries are produced.
