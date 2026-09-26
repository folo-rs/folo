# Publication and immutable intent

Publication delivers versions already chosen before merge. It does not increment
them, repair manifests or refresh committed dependency resolution.

## Capture intent once

A **publication manifest** is the immutable release request captured from a
clean, merged source commit and its committed configuration. It includes every
publishable package's exact declared version, even when version assessment
reports that package as unchanged.

The manifest records source identity, workspace locations, destination
configuration and each binary's executable name and targets. It is not a PR
version plan and contains no mutable "published" flags.

A **publication outcome** records one attempt at a phase, linked to the
manifest's identity. A **platform batch** is a frozen derivative describing the
binary work for one native target, including actual tag commits.

```text
clean merged source + configuration
  -> immutable publication manifest
       -> registry reconciliation -> attempt outcome
       -> GitHub reconciliation   -> attempt outcome + platform batches
                                                     -> binary attempt outcomes
```

This shows artifact flow, not concurrent permission to publish. Registry
completion is a prerequisite for GitHub writes.

Changing the source, requested versions or effective configuration requires a
new manifest at a distinct destination. A fetched release-branch tip is not part
of the manifest's intent digest: branch movement does not change what the
original source asked to publish.

## Reconcile exact versions

Registry publication observes every requested exact version. Existing versions
are not uploaded again. A yanked version still occupies its identity; the
publisher neither republishes nor unyanks it. A failed query is unknown state,
not evidence of absence.

Cargo verifies archives and orders missing workspace uploads. Assessment batches
are not an upload scheduler. Package normalization may remove inactive dependency
branches, but cannot introduce unassessed binary dependency identities.

Automatic uploads use GitHub Actions OIDC and crates.io Trusted Publishing.
Credentials are acquired per upload after package verification, rather than
assuming one short-lived credential can cover an entire cold build. There is no
stored-token or PAT fallback.

## Source, tags and native batches

The **publication source** is the merged commit whose versions are requested.
The **tag target** is the actual commit named by a package tag. A later commit
can be a valid tag target when the requested version and released content remain
equivalent. The release baseline used for planning is a separate identity.

Package tags use `{package}-v{version}` and are never moved. A missing tag is
created only at a verified eligible release-branch snapshot. Libraries receive
tags; binary packages also receive GitHub releases attached to established tags.

A batch records the actual **peeled tag commit**: the commit reached after
resolving an annotated or lightweight tag. Binaries build from that commit, not
from the version anchor, current branch tip or publication source by assumption.

The package `widget-cli` illustrates why package and executable names are separate:

```text
package/version: widget-cli 2.0.1
executable:      widget
tag:             widget-cli-v2.0.1
Windows assets:  widget-cli-v2.0.1-x86_64-pc-windows-msvc.zip
                 widget-cli-v2.0.1-x86_64-pc-windows-msvc.sha256
ZIP member:      widget.exe
```

This is naming shorthand, not a platform-batch schema.

Each target batches packages but builds them separately with their own default
feature selection. A release/target pair is complete only when both ZIP and
checksum assets are uploaded. An incomplete pair is repaired together.

## Retries preserve the request

Every attempt writes a new outcome. Original manifests and emitted batches
remain unchanged. Fresh remote observations determine work; a previous success
receipt does not prove that an asset still exists.

GitHub Actions outcomes carry optional run and attempt linkage. The final
reporter checks phase outcomes and current job results, not only artifact
identities. If GitHub reconciliation observes missing assets on a later attempt,
an earlier binary success cannot complete that work even when the regenerated
batch has an identical `batch_id`.

A dry run reports observations and intended work, never a completed publication.
Missing required artifacts are errors, not empty work sets. Successful uploads
are retained after a later failure.

Queued workflows do not serialize merges. A newer version can reach the release
branch before an older request has a tag. If automatic tag creation cannot
complete, that release's tag-dependent work is skipped, independent releases
continue, and the workflow **fails** with an operator handoff. The operator
creates the exact missing tag at the original publication source and retries
the original failed run. See [recovery](../operations/recovery.md#missing-tag-after-a-newer-version-merges).
