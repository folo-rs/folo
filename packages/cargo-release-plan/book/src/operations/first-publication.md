# First publication and adoption

The normal automatic path assumes established crates.io packages and a release
history that corresponds to their content. Introducing a new package and
adopting an existing repository need explicit maintainer setup.

## First publication happens before the first merge

crates.io Trusted Publishing cannot publish a package that does not yet exist,
because its publisher registration is configured on that existing package.
A maintainer performs the first publication from the feature branch **before
the package's first merge**.

1. Review and validate the new package and its publication dependencies.
2. Publish its bootstrap version manually, in dependency order.
3. Configure its crates.io Trusted Publisher for the consuming repository's
   calling workflow and optional environment.
4. Prepare a strictly higher version for the first merge, including exact-group
   alignment, dependent requirements and consistent lockfiles.
5. Update the PR's release explanation with both versions and the completed
   maintainer setup.
6. Merge under the repository's normal review policy. This performs the second
   publication: the first automated release.

For example, if introducing the running example as new packages:

| Package | Manual bootstrap | First merged automated version |
| --- | --- | --- |
| `widget_impl`, `widget` | `1.4.0` | `1.5.0` |
| `widget-cli` | `2.0.0` | `2.0.1` |
| `widget-fixtures` | Not published | Align to `1.5.0` |

These numbers illustrate the ordering requirement; choose versions appropriate
to the actual new packages. The helper needs no registry account or publisher.

The manual operation is ordinary Cargo publication, for example:

```powershell
cargo publish --package widget_impl
```

It belongs to an authorized maintainer, not tests, the local skill or an
unprivileged workflow. Follow crates.io's supported maintainer authentication
procedure; do not add persistent publication credentials to the release
automation.

Do not merge the bootstrap version unchanged. Do not rerun a release-branch
workflow expecting it to publish source that exists only on a feature branch.
For binary packages, the first automated merged version receives its GitHub
release and native assets.

## Registry preflight is not publisher verification

`check-published` without a plan discovers missing and unknown packages across
the workspace as an advisory. Before applying a resolved plan,
`check-published --plan <resolved-plan.json>` validates its targets and requires
an established registry package for every publishable target. Missing or unknown
registry results block that gate; nonpublishable alignment helpers make no query.

These observations do not verify Trusted Publisher registration. Complete that
setup explicitly before resuming. The separate `check-publishing-identity`
OIDC exchange/revocation probe tests the calling workflow identity without
uploading; it does not establish every package's grant either. A package can lack
a Git release anchor while its manually published bootstrap version already
exists in crates.io.

## Adopting an already-published workspace

Before enabling publication, establish correspondence between current versions,
their source and existing remote releases:

- Identify the source used to publish each current package version.
- Compare relevant package content and normalized publication inputs where
  needed, including binary dependency resolution.
- Verify repository ownership of package identities and existing package tags.
- Check that full first-parent history supports the intended anchors.
- Inspect existing binary releases against the configured naming and target
  contract.
- Run publication-input checks and read-only reconciliation before enabling
  writes.

Matching version strings or newly imported Git history are not proof of matching
released content. Resolve discrepancies through reviewed forward versions rather
than rewriting existing registry versions or tags.

When replacing another publisher, keep only one mutating release path active.
Preserve the caller workflow filename where existing Trusted Publisher
registrations depend on it. Drain or explicitly account for old queued runs;
changing a concurrency identity does not coordinate with those old runs.

Source-mode tests, dry runs and a successfully built book do not prove that the
published action/tool combination can perform the live path. Complete an
authorized acceptance release using the exact pinned combination before relying
on it for routine delivery.

## Emergency manual publication

When the automated registry path is unavailable, an authorized maintainer can
publish the exact reviewed merged versions with Cargo in dependency order.
Preserve their source identity and locked-resolution requirements.

Then resume reconciliation for that original source so tags, releases and binary
assets complete. Already uploaded versions are skipped. This does not require
another version increment solely because automation needs retrying.

Keep emergency access out of the normal automation. The automatic path retains
OIDC and its phase-specific permissions.
