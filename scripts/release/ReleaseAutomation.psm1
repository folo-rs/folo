# Release-automation logic for the `Release` GitHub workflow (.github/workflows/release.yml).
#
# The workflow steps are thin `just` recipes (gh-compose-release-config / gh-release /
# gh-plan-release-binaries in justfiles/just_automation.just) that import this module and call
# its functions, so the non-trivial logic lives here where it can be exercised by the Pester
# suite (ReleaseAutomation.Tests.ps1) against fixtures rather than only by pushing to `main`.
#
# The functions run real external tools where that is safe on fixtures (`cargo metadata`, file
# I/O) and isolate the ones that would touch crates.io / GitHub for real (`release-plz`, `gh`)
# behind small seams the tests mock.

Set-StrictMode -Version 3.0

function Get-ReleaseTarget {
    # The single source of truth for the triple -> runner mapping. The workflow's build matrix
    # is derived from this (via Get-MissingBinaryMatrix), so a target is added in exactly one
    # place. Native runners, one per target, no cross-compilation. GitHub offers `-latest` only
    # for x64 Linux/Windows and macOS (macos-latest is arm64); ARM Linux/Windows have no
    # `-latest` alias, so they are pinned by version. Intel macOS is intentionally absent.
    [CmdletBinding()]
    param()

    @(
        [pscustomobject]@{ Triple = 'x86_64-unknown-linux-gnu';  Os = 'ubuntu-latest' }
        [pscustomobject]@{ Triple = 'aarch64-unknown-linux-gnu'; Os = 'ubuntu-24.04-arm' }
        [pscustomobject]@{ Triple = 'x86_64-pc-windows-msvc';     Os = 'windows-latest' }
        [pscustomobject]@{ Triple = 'aarch64-pc-windows-msvc';    Os = 'windows-11-arm' }
        [pscustomobject]@{ Triple = 'aarch64-apple-darwin';       Os = 'macos-latest' }
    )
}

function Get-PublishableBinaryCrate {
    # Derives the crates this workflow releases: publishable to a registry AND owning a `bin`
    # target. In `cargo metadata` the `publish` field is null (any registry), an empty list
    # (never publish), or a non-empty registry list, so "publishable" is null-or-non-empty.
    # Returns {Name, Version} objects sorted by name. Runs the real `cargo metadata` (offline
    # with --no-deps); tests point it at a fixture workspace via -ManifestPath.
    [CmdletBinding()]
    param(
        [string] $ManifestPath
    )

    $cargoArgs = @('metadata', '--no-deps', '--format-version', '1')
    if ($ManifestPath) { $cargoArgs += @('--manifest-path', $ManifestPath) }

    $metadata = & cargo @cargoArgs | ConvertFrom-Json
    $metadata.packages |
        Where-Object { ($null -eq $_.publish) -or ($_.publish.Count -gt 0) } |
        Where-Object { $_.targets | Where-Object { $_.kind -contains 'bin' } } |
        ForEach-Object { [pscustomobject]@{ Name = $_.name; Version = $_.version } } |
        Sort-Object -Property Name -Unique
}

function Add-GitReleaseEnableFlag {
    # Pure line-based edit: returns a copy of $Line with `git_release_enable = true` set for each
    # crate in $CrateName. The committed release-plz.toml keeps releases off at the workspace
    # level (most crates are libraries); this turns it on only for the binary crates.
    #
    # `name = "<crate>"` is unique and is the first key of its [[package]] block, so inserting
    # right after that line lands the flag inside the block. The match is exact (trimmed) so
    # `cargo-bench-history` does not collide with `cargo-bench-history-core`. A crate with no
    # existing entry gets a fresh [[package]] block. Idempotent: an existing flag is left alone.
    [CmdletBinding()]
    param(
        [string[]] $Line,
        [string[]] $CrateName
    )

    $lines = [System.Collections.Generic.List[string]]::new()
    foreach ($item in $Line) { $lines.Add($item) }

    foreach ($crate in $CrateName) {
        $needle = 'name = "' + $crate + '"'
        $nameIndex = -1
        for ($i = 0; $i -lt $lines.Count; $i++) {
            if ($lines[$i].Trim() -eq $needle) { $nameIndex = $i; break }
        }

        if ($nameIndex -ge 0) {
            $alreadySet = $false
            for ($j = $nameIndex + 1; $j -lt $lines.Count; $j++) {
                $trimmed = $lines[$j].Trim()
                if ($trimmed.StartsWith('[')) { break }
                if ($trimmed -match '^git_release_enable\s*=') { $alreadySet = $true; break }
            }
            if (-not $alreadySet) {
                $lines.Insert($nameIndex + 1, 'git_release_enable = true')
            }
        } else {
            $lines.Add('')
            $lines.Add('[[package]]')
            $lines.Add($needle)
            $lines.Add('git_release_enable = true')
        }
    }

    # Comma keeps a single-line result an array rather than a bare string.
    , $lines.ToArray()
}

function New-ReleasePlzConfig {
    # Reads the committed release-plz.toml at $SourcePath, injects `git_release_enable = true`
    # for each $CrateName, and writes the result to $OutputPath. The output is UTF-8 without a
    # BOM and uses LF line endings with a trailing newline, deterministically on every platform,
    # so the artifact does not vary with the runner OS. The caller writes this outside the
    # working tree so `cargo publish` never sees a dirty repo.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $SourcePath,
        [Parameter(Mandatory)][string] $OutputPath,
        [string[]] $CrateName
    )

    $sourceLines = [System.IO.File]::ReadAllText($SourcePath) -split "`r?`n"
    $newLines = Add-GitReleaseEnableFlag -Line $sourceLines -CrateName $CrateName
    $content = ($newLines -join "`n").TrimEnd("`n") + "`n"
    [System.IO.File]::WriteAllText($OutputPath, $content, [System.Text.UTF8Encoding]::new($false))
}

function Get-BinaryReleaseAsset {
    # Returns the names of the assets already attached to the GitHub release for $Tag, or $null
    # if no such release exists yet. Isolates the real `gh release view` call so the tests can
    # mock it.
    #
    # A non-zero `gh` exit is treated as "no release yet" ONLY when it is the specific "release
    # not found" case; any other failure (auth, network, GitHub API error) is rethrown. Swallowing
    # those would let the caller build an empty/partial matrix, so the binary build is skipped and
    # the workflow looks successful while binaries are still missing.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $Tag
    )

    # Disable the native-error preference locally so a non-zero exit does not terminate here
    # before we can classify it; we inspect the exit code and output ourselves. 2>&1 merges
    # stderr (where gh prints "release not found") into the captured output.
    $PSNativeCommandUseErrorActionPreference = $false
    $output = gh release view $Tag --json assets 2>&1
    $exitCode = $LASTEXITCODE

    if ($exitCode -ne 0) {
        $text = ($output | Out-String).Trim()
        if ($text -match 'release not found') { return $null }
        throw "gh release view '$Tag' failed (exit $exitCode): $text"
    }

    # An existing-but-empty release returns @() (all targets missing), distinct from $null
    # ("no release yet", skip). The guard also keeps member enumeration strict-mode-safe.
    $parsed = ($output | Out-String) | ConvertFrom-Json
    if (-not $parsed.assets) { return , @() }
    , @($parsed.assets.name)
}

function Get-MissingBinaryMatrix {
    # Reconciles desired vs. actual binary assets. For each crate it computes the expected tag
    # `{Name}-v{Version}` and, when that release exists, the per-target archives
    # `{Name}-v{Version}-{triple}.zip`; every expected archive not already uploaded becomes a
    # matrix row {name, version, tag, triple, os}. This is what makes the workflow self-healing:
    # a re-run rebuilds only what is still missing, from the actual published state, with no
    # hand-maintained crate list.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][object[]] $Crate,
        [object[]] $Target = (Get-ReleaseTarget)
    )

    $rows = [System.Collections.Generic.List[object]]::new()
    foreach ($crate in $Crate) {
        $tag = "$($crate.Name)-v$($crate.Version)"
        $assets = Get-BinaryReleaseAsset -Tag $tag
        if ($null -eq $assets) { continue }

        foreach ($target in $Target) {
            $archive = "$($crate.Name)-v$($crate.Version)-$($target.Triple).zip"
            if ($assets -contains $archive) { continue }
            $rows.Add([pscustomobject]@{
                    name    = $crate.Name
                    version = $crate.Version
                    tag     = $tag
                    triple  = $target.Triple
                    os      = $target.Os
                })
        }
    }

    , $rows.ToArray()
}

function ConvertTo-MatrixJson {
    # Renders matrix rows as the compact JSON array that `fromJSON` in the workflow consumes.
    # ConvertTo-Json unwraps a single-element array to a bare object, so a one-row result is
    # re-wrapped; an empty result is the literal `[]`.
    [CmdletBinding()]
    param(
        [object[]] $Row
    )

    if (-not $Row -or $Row.Count -eq 0) { return '[]' }

    $json = ConvertTo-Json -InputObject @($Row) -Compress -Depth 5
    if ($json.TrimStart().StartsWith('[')) { $json } else { "[$json]" }
}

function Invoke-WithRetry {
    # Runs $Action, retrying up to $Attempt times with $DelaySeconds between tries; rethrows the
    # last failure if every attempt fails. crates.io rate-limits and the Trusted-Publishing OIDC
    # token is short-lived, so the publish is retried rather than failing the whole release on a
    # transient rejection.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][scriptblock] $Action,
        [int] $Attempt = 3,
        [int] $DelaySeconds = 900
    )

    for ($n = 1; $n -le $Attempt; $n++) {
        try {
            & $Action
            return
        } catch {
            if ($n -eq $Attempt) { throw }
            Write-Warning "Attempt $n of $Attempt failed: $($_.Exception.Message). Retrying in $DelaySeconds seconds..."
            Start-Sleep -Seconds $DelaySeconds
        }
    }
}

function Invoke-ReleasePublish {
    # Publishes changed crates to crates.io via `release-plz release` using the composed CI
    # config, with bounded retries. release-plz is idempotent (it skips already-published
    # versions), so a retry or a whole re-run safely resumes a partially-published release. NOT
    # for local use: it performs real publishes. The native-error preference is disabled locally
    # so a non-zero exit is handled here (turned into a retryable failure) rather than aborting.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $ConfigPath,
        [int] $Attempt = 3,
        [int] $DelaySeconds = 900
    )

    $PSNativeCommandUseErrorActionPreference = $false
    Invoke-WithRetry -Attempt $Attempt -DelaySeconds $DelaySeconds -Action {
        release-plz release --config $ConfigPath
        if ($LASTEXITCODE -ne 0) {
            throw "release-plz release exited with code $LASTEXITCODE"
        }
    }
}

function Set-GitHubOutput {
    # Emits a `name=value` step output for the workflow (and echoes it for the run log). No-ops
    # the file append when GITHUB_OUTPUT is unset, so the recipes are runnable locally.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $Name,
        [Parameter(Mandatory)][string] $Value
    )

    Write-Host "$Name=$Value"
    if ($env:GITHUB_OUTPUT) {
        Add-Content -Path $env:GITHUB_OUTPUT -Value "$Name=$Value" -Encoding utf8
    }
}

Export-ModuleMember -Function `
    Get-ReleaseTarget, `
    Get-PublishableBinaryCrate, `
    Add-GitReleaseEnableFlag, `
    New-ReleasePlzConfig, `
    Get-BinaryReleaseAsset, `
    Get-MissingBinaryMatrix, `
    ConvertTo-MatrixJson, `
    Invoke-WithRetry, `
    Invoke-ReleasePublish, `
    Set-GitHubOutput
