# Verify delivery

Version readiness and delivered availability are different checks. Start with
the original publication manifest and the outcomes linked to it, then confirm
the remote state you depend on.

## Operator checklist

- The manifest identifies the intended repository, source commit and workspace.
- Every requested exact package version is available on crates.io.
- Every package tag resolves to a validated commit and no existing tag was moved.
- Every binary package has a GitHub release attached to its established tag.
- Each selected native target has both archive and checksum assets uploaded.
- The ZIP contains the expected executable, not a name inferred from the package.
- Any dry-run, blocked, failed or unknown outcome remains distinguishable from
  completed delivery.

Nonpublishable alignment helpers do not appear as registry requests. Packages
unchanged against their merged anchors still do.

## Read outcomes correctly

The registry outcome's `publication_id` links it to the manifest. Its
`dry_run` and `complete` fields distinguish planning from completion; inspect
per-package states and diagnostics too.

An existing exact version is not uploaded again. A yanked version still exists,
but presence alone does not prove that Cargo can use it for a required dependency.
A query failure must not be interpreted as absence.

Outcomes record what one attempt observed. They are not permanent proof of remote
availability, and a subsequent attempt refreshes observations.

In GitHub Actions, preserve the optional `github.run_id` and
`github.run_attempt` metadata. Binary receipts must match their publication and
batch identities and be at least as new as the GitHub reconciliation that
requested them. An identical `batch_id` does not make an old binary success
evidence for newly missing assets.

Use `publish report` with the downloaded `outcome.json` artifact subdirectories
and current job results to assess the whole run. It still requires GitHub run
context with `--no-issue`; that option suppresses issue writes only. A missing
publication manifest remains an incomplete release even if individual jobs
succeeded. See the [reporting example](../integration/publication.md#report-every-attempt).

## Inspect tags and assets

The following read-only GitHub query uses the running example; replace its
repository and tag:

```powershell
$Repository = "example/widgets"
$Tag = "widget-cli-v2.0.1"
gh release view $Tag --repo $Repository --json tagName,assets
```

Inspect the remote tag, including its peeled commit when annotated:

```powershell
git ls-remote origin "refs/tags/$Tag" "refs/tags/$Tag^{}"
```

A release's displayed branch name is not a substitute for resolving the actual
tag. Compare the result with the batch's recorded source commit. An established
tag may name a later release-equivalent source than the registry-publication
commit; that does not make current branch HEAD the binary source.

## Inspect the ZIP and checksum

For one selected target, download the pair into a fresh workspace-local directory:

```powershell
$Stem = "widget-cli-v2.0.1-x86_64-pc-windows-msvc"
$Destination = Join-Path ".release-plan-work" "verify-windows"
gh release download $Tag --repo $Repository --pattern "$Stem.*" `
    --dir $Destination
$Archive = Join-Path $Destination "$Stem.zip"
Get-FileHash $Archive -Algorithm SHA256
Get-Content (Join-Path $Destination "$Stem.sha256")
$Contents = Join-Path $Destination "contents"
Expand-Archive $Archive -DestinationPath $Contents
Get-ChildItem $Contents
```

Compare the computed digest with the sidecar. This ZIP should contain
`widget.exe` at its root; on Unix targets the member is `widget` with executable
permissions. The package name still controls the tag and asset names.

The sidecar supports explicit verification. Do not assume that ordinary
cargo-binstall installation discovers it automatically or that a checksum proves
an independent publisher identity.

Test the intended installation path in a clean installation root and run the
binary's documented harmless smoke operation. Binstall's normal source fallback
is not an archive test: when verifying a promised native archive, require binary
installation without fallback using the options supported by your pinned
binstall release.

For incomplete pairs, use [recovery](recovery.md). Do not choose a new package
version merely to replace an asset missing from an existing release.
