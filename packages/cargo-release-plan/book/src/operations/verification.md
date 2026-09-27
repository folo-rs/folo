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

Non-publishable version targets do not appear as registry requests. Packages
unchanged against their merged anchors still do.

## Read outcomes correctly

The registry outcome's `publication_id` links it to the manifest. Its
`dry_run` and `complete` fields distinguish observation-only attempts from completion; inspect
per-package states and diagnostics too.

An existing exact version is not uploaded again. A yanked version still exists,
but presence alone does not prove that Cargo can use it for a required dependency.
A query failure must not be interpreted as absence.

Outcomes record what one attempt observed. They are not permanent proof of remote
availability, and a subsequent attempt refreshes observations.

In GitHub Actions, preserve the optional `github.run_id` and
`github.run_attempt` metadata. Check
[outcome identity and freshness](../reference/artifacts.md#derived-batches-and-later-outcomes)
before treating an earlier attempt as evidence for current work.

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
git ls-remote "https://github.com/$Repository.git" "refs/tags/$Tag" "refs/tags/$Tag^{}"
```

A release's displayed branch name is not a substitute for resolving the actual
tag. Compare the result with the batch's recorded source commit. An established
tag may name a later release-equivalent source than the registry-publication
commit; that does not make current branch HEAD the binary source.

## Inspect the ZIP and checksum

For the Windows target in this example, download the pair into a fresh
workspace-local directory:

```powershell
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
$Stem = "widget-cli-v2.0.1-x86_64-pc-windows-msvc"
$Destination = Join-Path ".release-plan-work" "verify-windows"
gh release download $Tag --repo $Repository --pattern "$Stem.*" `
    --dir $Destination
$Archive = Join-Path $Destination "$Stem.zip"
# GNU-compatible checksum lines put the digest before the marker and archive filename.
$Expected = (Get-Content (Join-Path $Destination "$Stem.sha256") -Raw).Split()[0]
$Actual = (Get-FileHash $Archive -Algorithm SHA256).Hash
if ($Expected -notmatch '\A[0-9a-fA-F]{64}\z' -or $Actual -ine $Expected) {
    throw "The archive does not match its SHA-256 sidecar."
}
$Contents = Join-Path $Destination "contents"
Expand-Archive $Archive -DestinationPath $Contents
Get-ChildItem $Contents
```

The comparison stops before extraction on a mismatch. This Windows ZIP should
contain `widget.exe` at its root. The package name controls the tag and asset names.

For a Unix-target archive, perform the same digest comparison, then inspect its
stored mode using PowerShell's .NET ZIP reader:

```powershell
$Zip = [IO.Compression.ZipFile]::OpenRead($Archive)
try {
    foreach ($Entry in $Zip.Entries) {
        [pscustomobject]@{
            Name = $Entry.FullName
            UnixMode = [Convert]::ToString(($Entry.ExternalAttributes -shr 16) -band 0x1ff, 8)
        }
    }
} finally {
    $Zip.Dispose()
}
```

`UnixMode` selects the permission bits from ZIP external attributes. Confirm that
the root member is `widget` and its stored mode includes executable permissions.
Use an extraction or installation path that preserves those permissions.
`Expand-Archive` does not verify the stored Unix mode through the extracted file,
so inspect the archive metadata rather than treating that extraction as permission
evidence.

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
