# Recover a failed release

You do not need to inspect tags, archives or registry entries after every
successful release. The shared workflow owns those checks. Use this chapter when
GitHub reports a failure and you need to decide what to retry or repair.

## Find out what failed

Before merge, a failed PR check appears on the pull request. Fix the reported
problem before merging; these checks do not start publication.

After merge, an incomplete release marks its workflow and affected commit checks
as failed. The final reporter creates or updates an issue titled
**Release failed: workflow run ...**, linking the run and describing incomplete
packages and any required operator action. A failed retry updates that same
issue rather than creating a new issue for each attempt.

Start with the issue's instructions and the failed job's logs. If there is no
issue, open the failed or cancelled run in the repository's **Actions** tab.
Cancellation, startup failure or missing reporting permissions can prevent an
issue from being created; its absence does not mean the release succeeded.

## Choose the recovery action

| Reported problem | What to do |
| --- | --- |
| Temporary network, registry or runner failure | Wait for the service to recover, then rerun the original failed jobs. |
| Publishing permission or Trusted Publisher registration is wrong | Have a maintainer correct the setting, then retry. If the fix changes workflow code, use a new recovery run as described below. |
| A native build prerequisite is unavailable | Repair the build environment. Retry the original run if it can use that repair; use a new recovery run if updated automation is required. |
| A ZIP or checksum upload is incomplete | Rerun the original failed jobs. The publisher completes missing work and repairs an incomplete pair together. |
| The failure issue names a missing tag after another version merged | Create that exact tag at the recorded source, then retry the original failed jobs. |
| Required artifacts expired, or the publisher needs corrected automation | Start a new recovery run for the original source. Do not reconstruct or edit the old artifacts. |
| The released package's source is defective | Correct it in a new PR, run the skill and release a new version. Retrying unchanged source will not fix it. |
| An existing tag or uploaded version refers to unexpected content | Stop automatic retries and investigate. Do not move the tag or overwrite the version to make the run green. |

## Retry the original run when its request is still correct

In **Actions**, open the original failed release run and choose **Re-run failed
jobs** after correcting the cause. Rerunning the whole original workflow is also
safe when that is necessary: it still requests the original source and versions.

Already published versions, established tags and complete archive pairs are
preserved. Retrying an upload failure therefore does not normally need a new
package version. Independent packages may have completed even though the overall
run failed.

A retry does not update the run to newer source or newer workflow code. If that
is what the repair requires, choose the appropriate path below rather than
repeatedly rerunning the same failing inputs.

When a retry succeeds, close its failure issue with a link to the successful
attempt. Successful retries do not automatically close those issues.

## Missing tag after a newer version merges

For example, the run for `widget-cli 2.0.1` can finish its crate upload after
`2.0.2` has already merged. The old run still needs the `2.0.1` tag, but cannot
automatically tag the newer source as that version.

The failure issue identifies the exact tag and original source commit. An
authorized maintainer must create that tag at the recorded commit, not at the
current branch tip. Do not move an existing tag.

These commands are for that specific manual repair. Replace the example values
with those from the failure issue:

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true

$Repository = "example/widgets"
$Tag = "widget-cli-v2.0.1"
$Source = "<original-source-SHA-from-the-failure-issue>"
$RepositoryUrl = "https://github.com/$Repository.git"

# Inspect the recorded source and check whether the tag already exists.
git fetch $RepositoryUrl $Source
git show --no-patch --format=fuller $Source
git ls-remote $RepositoryUrl "refs/tags/$Tag" "refs/tags/$Tag^{}"
```

Only after confirming the intended source and that the tag is absent, create it:

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true

git tag --annotate $Tag $Source --message "Release $Tag"
git push $RepositoryUrl "refs/tags/$Tag"
```

Then use **Re-run failed jobs** on the original run. It recognizes the created
tag and completes the remaining work from that source. If the tag already exists
but identifies different content, investigate instead of force-updating it.

## Start a new recovery run when retrying is insufficient

If the original artifacts are unavailable or a fix requires newer publishing
automation, open the release workflow in **Actions** and choose **Run workflow**.
Select the release branch and set its `source` input to the **full original
publication-source commit** from the failure issue or failed run.

This uses the release branch's current automation to create a new publication
request for that original source and its configuration. It observes existing
uploads and completes what is still missing. It does not recreate the old
outcomes or turn the earlier failed run into a successful attempt.
If the original source cannot be restored, do not substitute different content
under the same versions; prepare a new release instead.

The caller needs the recovery input shown in
[Connect publication](../integration/publication.md#add-the-release-caller).
If your caller does not expose it, have a maintainer update the caller rather
than assembling publication commands or editing artifacts manually.

If the original publication configuration itself was wrong, selecting the same
source also selects that old configuration. Correct the configuration through a
reviewed PR and let its normal release run issue a new request instead. Do not
edit an existing publication manifest to change its targets or destination.

After a successful replacement run, close the original failure issue with a link
and explain that the new run supersedes it. The original failed run remains an
accurate record of the request it could not complete.

## When a new version is necessary

Create a forward release when the repair changes released package content or
when an occupied version cannot describe the intended code. Ask the agent to
make the correction and run `increment-versions`; review its proposed versions
normally.

A transient service failure, missing archive, permission repair or
automation-only correction does not by itself require new package versions.
The skill determines whether a proposed source/configuration correction actually
changes released content. A newer successful release does not retroactively
repair an older defective one; record that the old failure is superseded rather
than retrying it indefinitely.

## Release-branch movement

If a PR's version check fails because release history or its parent advanced,
ask the agent to refresh its branch and rerun `increment-versions`. It preserves
your source changes, reassesses against the new history and updates the release
table. Do not compensate by manually increasing stale proposed versions.
