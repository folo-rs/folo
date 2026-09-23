#requires -Version 7

# Release-automation logic for the `Release` GitHub workflow (.github/workflows/release.yml)
# and the local `just check-never-published` recipe.
#
# The workflow steps and release recipes are thin `just` wrappers (in justfiles/just_release.just)
# that import this module and call its functions, so the
# non-trivial logic lives here where it can be exercised by the Pester suite
# (ReleaseAutomation.Tests.ps1) against fixtures rather than only by pushing to `main`.
#
# The functions run real external tools where that is safe on fixtures (`cargo metadata`, file
# I/O) and isolate the ones that would touch crates.io / GitHub for real (`release-plz`, `gh`)
# behind small seams the tests mock.

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true

# The transient-fault retry (used by Invoke-ReleasePublish) is the shared workspace helper rather
# than a private copy, so every network-facing script retries the same way.
Import-Module (Join-Path $PSScriptRoot '..' 'utility' 'Retry.psm1') -Force

function Get-ReleaseTarget {
    # The single source of truth for the triple -> runner mapping. The workflow's build matrix
    # is derived from this (via release-binaries), so a target is added in exactly one
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

function Get-DeclaredReleaseTarget {
    # The target triples a crate restricts its prebuilt binaries to, read from its manifest's
    # `[package.metadata.folo] release-targets`. Returns an empty array when the crate declares
    # nothing, which means every target in Get-ReleaseTarget - the default, and what a portable
    # crate wants. A crate that only functions on some platforms names that subset so the workflow
    # does not publish archives whose binary could never run. Takes a `cargo metadata` package
    # object; StrictMode makes an absent property throw, so every hop is guarded explicitly.
    # PowerShell unrolls a single-element result, so callers wrap the call in @().
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][object] $Package
    )

    if ($Package.PSObject.Properties.Name -notcontains 'metadata') { return @() }
    if ($null -eq $Package.metadata) { return @() }
    if ($Package.metadata.PSObject.Properties.Name -notcontains 'folo') { return @() }

    $folo = $Package.metadata.folo
    if ($null -eq $folo) { return @() }
    if ($folo.PSObject.Properties.Name -notcontains 'release-targets') { return @() }

    @($folo.'release-targets')
}

function Get-BinaryTarget {
    # Returns the Cargo binary targets declared by a package metadata object. Keeping this
    # extraction in one function lets release planning and validation agree on what a binary
    # package contains.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][object] $Package
    )

    if ($Package.PSObject.Properties.Name -notcontains 'targets' -or
        $null -eq $Package.targets) {
        return @()
    }
    @($Package.targets | Where-Object { $_.kind -contains 'bin' })
}

function Test-PathCaseInsensitive {
    # Cargo opens manifests through the filesystem while Git pathspecs are case-sensitive by
    # default. Probe the workspace directory instead of inferring its behavior from the operating
    # system; an inconclusive probe keeps the stricter case-sensitive result.
    param(
        [Parameter(Mandatory)][string] $Directory
    )

    try {
        $entryName = @(
            Get-ChildItem -LiteralPath $Directory -Force -ErrorAction Stop |
                ForEach-Object { $_.Name }
        )
    } catch {
        return $false
    }
    $present = [System.Collections.Generic.HashSet[string]]::new(
        [StringComparer]::Ordinal
    )
    foreach ($name in $entryName) {
        [void] $present.Add($name)
    }
    foreach ($name in $entryName) {
        $flippedBuilder = [Text.StringBuilder]::new($name.Length)
        foreach ($character in $name.ToCharArray()) {
            if ([char]::IsUpper($character)) {
                [void] $flippedBuilder.Append([char]::ToLowerInvariant($character))
            } elseif ([char]::IsLower($character)) {
                [void] $flippedBuilder.Append([char]::ToUpperInvariant($character))
            } else {
                [void] $flippedBuilder.Append($character)
            }
        }
        $flipped = $flippedBuilder.ToString()
        if ($flipped -ceq $name -or $present.Contains($flipped)) {
            continue
        }
        return Test-Path -LiteralPath (Join-Path $Directory $flipped)
    }
    return $false
}

function Get-WorkspaceMember {
    # Returns current Cargo workspace members with publication eligibility and manifest identity.
    # Tracking is opt-in because the increment publication gate needs it, while ordinary release
    # discovery retains its Cargo-defined scope and must not gain a Git failure boundary.
    [CmdletBinding()]
    param(
        [string] $ManifestPath,
        [switch] $IncludeTracking
    )

    $cargoArgs = @('metadata', '--no-deps', '--format-version', '1')
    if ($ManifestPath) { $cargoArgs += @('--manifest-path', $ManifestPath) }

    $configuredTargetDirectory =
        [Environment]::GetEnvironmentVariable('CARGO_TARGET_DIR', 'Process')
    try {
        # Cargo rejects an explicitly present empty value. Treat it as the absence it represents
        # for this subprocess without changing the caller's environment permanently.
        if ($null -ne $configuredTargetDirectory -and
            $configuredTargetDirectory.Length -eq 0) {
            Remove-Item Env:CARGO_TARGET_DIR
        }
        $metadata = & cargo @cargoArgs | ConvertFrom-Json
    } finally {
        if ($null -ne $configuredTargetDirectory) {
            [Environment]::SetEnvironmentVariable(
                'CARGO_TARGET_DIR',
                $configuredTargetDirectory,
                'Process'
            )
        }
    }
    $workspaceMemberId = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($id in $metadata.workspace_members) {
        [void] $workspaceMemberId.Add([string] $id)
    }

    $workspaceRoot = [IO.Path]::GetFullPath([string] $metadata.workspace_root)
    $repositoryRoot = $null
    $workspacePrefix = $null
    $caseInsensitivePath = $false
    if ($IncludeTracking) {
        $previousNativeErrorPreference = $PSNativeCommandUseErrorActionPreference
        try {
            $PSNativeCommandUseErrorActionPreference = $false
            $gitOutput = @(& git -C $workspaceRoot rev-parse --show-toplevel 2>&1)
            $gitExitCode = $LASTEXITCODE
        } finally {
            $PSNativeCommandUseErrorActionPreference = $previousNativeErrorPreference
        }
        if ($gitExitCode -ne 0) {
            $diagnostic = @(
                $gitOutput | ForEach-Object { $_.ToString() }
            ) -join [Environment]::NewLine
            if ([string]::IsNullOrWhiteSpace($diagnostic)) {
                $diagnostic = '(no diagnostic output)'
            }
            throw (
                "git rev-parse failed while resolving the repository for workspace " +
                "'$workspaceRoot' with exit code $gitExitCode`: $diagnostic"
            )
        }
        $repositoryRootLine = @(
            $gitOutput |
                ForEach-Object { $_.ToString() } |
                Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
        )
        if ($repositoryRootLine.Count -ne 1) {
            throw (
                "git rev-parse returned an invalid repository root for workspace " +
                "'$workspaceRoot'."
            )
        }
        $repositoryRoot = [IO.Path]::GetFullPath($repositoryRootLine[0])

        $previousNativeErrorPreference = $PSNativeCommandUseErrorActionPreference
        try {
            $PSNativeCommandUseErrorActionPreference = $false
            $gitOutput = @(& git -C $workspaceRoot rev-parse --show-prefix 2>&1)
            $gitExitCode = $LASTEXITCODE
        } finally {
            $PSNativeCommandUseErrorActionPreference = $previousNativeErrorPreference
        }
        if ($gitExitCode -ne 0) {
            $diagnostic = @(
                $gitOutput | ForEach-Object { $_.ToString() }
            ) -join [Environment]::NewLine
            if ([string]::IsNullOrWhiteSpace($diagnostic)) {
                $diagnostic = '(no diagnostic output)'
            }
            throw (
                "git rev-parse failed while resolving the workspace prefix for " +
                "'$workspaceRoot' with exit code $gitExitCode`: $diagnostic"
            )
        }
        $workspacePrefixLine = @(
            $gitOutput |
                ForEach-Object { $_.ToString() } |
                Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
        )
        if ($workspacePrefixLine.Count -gt 1) {
            throw (
                "git rev-parse returned an invalid workspace prefix for " +
                "'$workspaceRoot'."
            )
        }
        $workspacePrefix = if ($workspacePrefixLine.Count -eq 0) {
            ''
        } else {
            $workspacePrefixLine[0].TrimEnd('/', '\')
        }
        $caseInsensitivePath = Test-PathCaseInsensitive -Directory $workspaceRoot
    }

    foreach ($package in $metadata.packages | Sort-Object -Property name) {
        if (-not $workspaceMemberId.Contains([string] $package.id)) {
            continue
        }

        $packageManifestPath = [IO.Path]::GetFullPath([string] $package.manifest_path)
        $tracked = $null
        if ($IncludeTracking) {
            # Cargo's workspace root and package manifests share Cargo's path spelling. Rebase
            # their relative relationship through Git's workspace prefix instead of subtracting
            # Git's independently spelled repository root from a Cargo path. This also retains
            # leading parent components for supported sibling members.
            $workspaceRelativeManifestPath =
                [IO.Path]::GetRelativePath($workspaceRoot, $packageManifestPath)
            $gitWorkspacePath = [IO.Path]::GetFullPath(
                [IO.Path]::Combine($repositoryRoot, $workspacePrefix)
            )
            $gitManifestPath = [IO.Path]::GetFullPath(
                [IO.Path]::Combine($gitWorkspacePath, $workspaceRelativeManifestPath)
            )
            $relativeManifestPath =
                [IO.Path]::GetRelativePath($repositoryRoot, $gitManifestPath)
            $outsideRepository =
                [IO.Path]::IsPathRooted($relativeManifestPath) -or
                $relativeManifestPath -eq '..' -or
                $relativeManifestPath.StartsWith(
                    "..$([IO.Path]::DirectorySeparatorChar)",
                    [StringComparison]::Ordinal
                )
            if ($outsideRepository) {
                $tracked = $false
            } else {
                # Git pathspecs are relative to -C and accept slash separators on every
                # supported host. Explicit literal magic prevents manifest directory names from
                # being interpreted as patterns; `icase` follows a case-insensitive checkout.
                $gitPath = $relativeManifestPath.Replace('\', '/')
                $gitPathspec = if ($caseInsensitivePath) {
                    ":(icase,literal)$gitPath"
                } else {
                    ":(literal)$gitPath"
                }
                $previousNativeErrorPreference = $PSNativeCommandUseErrorActionPreference
                try {
                    $PSNativeCommandUseErrorActionPreference = $false
                    $gitOutput = @(
                        & git -C $repositoryRoot ls-files --error-unmatch -- $gitPathspec 2>&1
                    )
                    $gitExitCode = $LASTEXITCODE
                } finally {
                    $PSNativeCommandUseErrorActionPreference = $previousNativeErrorPreference
                }
                switch ($gitExitCode) {
                    0 { $tracked = $true }
                    1 { $tracked = $false }
                    default {
                        $diagnostic = @(
                            $gitOutput | ForEach-Object { $_.ToString() }
                        ) -join [Environment]::NewLine
                        if ([string]::IsNullOrWhiteSpace($diagnostic)) {
                            $diagnostic = '(no diagnostic output)'
                        }
                        throw (
                            "git ls-files failed while checking workspace manifest " +
                            "'$relativeManifestPath' with exit code $gitExitCode`: $diagnostic"
                        )
                    }
                }
            }
        }

        [pscustomobject]@{
            Name         = [string] $package.name
            Version      = [string] $package.version
            ManifestPath = $packageManifestPath
            Publishable  = ($null -eq $package.publish) -or ($package.publish.Count -gt 0)
            Tracked      = $tracked
            Package      = $package
        }
    }
}

function Get-TrackedWorkspaceMember {
    # The current workspace members whose manifests Git tracks. Version-group membership remains
    # cargo-release-plan's responsibility; this projection only secures the publication gate.
    [CmdletBinding()]
    param(
        [string] $ManifestPath
    )

    Get-WorkspaceMember -ManifestPath $ManifestPath -IncludeTracking |
        Where-Object Tracked
}

function Get-PublishableBinaryCrate {
    # Derives the crates this workflow releases: Cargo workspace members publishable to a registry
    # AND owning a `bin` target. In `cargo metadata` the `publish` field is null (any registry), an
    # empty list (never publish), or a non-empty registry list.
    # Returns {Name, Version, Binary, ReleaseTargets} objects sorted by name, where Binary is the
    # package's single binary target and ReleaseTargets is its declared release-target restriction
    # (empty for the usual "all targets" case). A release archive has one binary path, so packages
    # with several binary targets are rejected rather than silently publishing only one. Runs real
    # Cargo metadata; tests point it at a fixture via -ManifestPath.
    [CmdletBinding()]
    param(
        [string] $ManifestPath
    )

    Get-WorkspaceMember -ManifestPath $ManifestPath |
        Where-Object Publishable |
        Where-Object { $_.Package.targets | Where-Object { $_.kind -contains 'bin' } } |
        ForEach-Object {
            $binaryTargets = @(Get-BinaryTarget -Package $_.Package)
            if ($binaryTargets.Count -ne 1) {
                throw (
                    "Publishable binary package '$($_.Name)' declares $($binaryTargets.Count) " +
                    'binary targets; release automation requires exactly one.'
                )
            }
            [pscustomobject]@{
                Name           = $_.Name
                Version        = $_.Version
                Binary         = [string] $binaryTargets[0].name
                ReleaseTargets = @(Get-DeclaredReleaseTarget -Package $_.Package)
            }
        } |
        Sort-Object -Property Name -Unique
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
        [Parameter(Mandatory)][string] $Tag,
        [string] $Repository
    )

    # Disable the native-error preference locally so a non-zero exit does not terminate here
    # before we can classify it; we inspect the exit code and output ourselves. 2>&1 merges
    # stderr (where gh prints "release not found") into the captured output.
    $PSNativeCommandUseErrorActionPreference = $false
    $arguments = @('release', 'view', $Tag, '--json', 'assets')
    if ($Repository) { $arguments += @('--repo', $Repository) }
    $output = & gh @arguments 2>&1
    $exitCode = $LASTEXITCODE

    if ($exitCode -ne 0) {
        $text = ($output | Out-String).Trim()
        if ($text -match 'release not found') { return $null }
        throw "gh release view '$Tag' failed (exit $exitCode): $text"
    }

    # An existing-but-empty release returns @() (all target asset pairs missing), distinct from
    # $null ("no release yet"). The guard also keeps member enumeration strict-mode-safe.
    $parsed = ($output | Out-String) | ConvertFrom-Json
    if (-not $parsed.assets) { return , @() }
    , @($parsed.assets.name)
}

function Invoke-ReleasePublish {
    # Publishes changed crates to crates.io via `release-plz release` using the registry-only
    # config, with bounded retries. release-plz is idempotent (it skips already-published
    # versions), so a retry or a whole re-run safely resumes a partially-published release. NOT
    # for local use: it performs real publishes. The native-error preference is disabled locally
    # so a non-zero exit is handled here (turned into a retryable failure) rather than aborting.
    [Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSReviewUnusedParameter', 'ConfigPath',
        Justification = 'Consumed inside the -Action retry closure (release-plz --config), which the rule does not trace into.')]
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

function Get-PublishableCrate {
    # Every Cargo workspace crate publishable to a registry (unlike Get-PublishableBinaryCrate,
    # not filtered to binaries), as {Name, Version} objects sorted by name. Used by the
    # never-published preflight, which must warn about any brand-new crate, library or binary.
    # Runs real Cargo metadata; tests point it at a fixture workspace via -ManifestPath.
    [CmdletBinding()]
    param(
        [string] $ManifestPath
    )

    Get-WorkspaceMember -ManifestPath $ManifestPath |
        Where-Object Publishable |
        ForEach-Object { [pscustomobject]@{ Name = $_.Name; Version = $_.Version } } |
        Sort-Object -Property Name -Unique
}

function Get-CrateIndexPath {
    # crates.io sparse-index path for a crate, keyed by (lowercased) name length. Pure, so the
    # length-branch logic is unit-tested without touching the network. 1 and 2-char names live
    # under `1/` and `2/`; 3-char under `3/<first-letter>/`; everything else under
    # `<first-two>/<next-two>/`.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $Name
    )

    $n = $Name.ToLowerInvariant()
    switch ($n.Length) {
        1 { "1/$n" }
        2 { "2/$n" }
        3 { "3/$($n.Substring(0, 1))/$n" }
        default { "$($n.Substring(0, 2))/$($n.Substring(2, 2))/$n" }
    }
}

function Get-CratePublishStatus {
    # Best-effort crates.io presence check for one crate. Returns 'Published' (HTTP 200),
    # 'NeverPublished' (HTTP 404), or 'Unknown' (a transient rate-limit / 5xx / network error).
    # Isolates the single HTTP call so the preflight loop and its tests stay off the network.
    # -SkipHttpErrorCheck stops Invoke-WebRequest throwing on 4xx/5xx so 404 is classified rather
    # than caught; the try/catch handles genuine network failures.
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string] $Name
    )

    $url = "https://index.crates.io/$(Get-CrateIndexPath -Name $Name)"
    try {
        $response = Invoke-WebRequest -Uri $url -Method Get -SkipHttpErrorCheck
    } catch {
        return 'Unknown'
    }

    switch ([int] $response.StatusCode) {
        200 { 'Published' }
        404 { 'NeverPublished' }
        default { 'Unknown' }
    }
}

function Test-NeverPublishedCrate {
    # Preflight for the `increment-versions` skill: warns about publishable crates that crates.io
    # has never seen. Trusted Publishing cannot perform a crate's first-ever publish (the crate
    # must already exist so a trusted publisher can be configured on it), so a brand-new crate's
    # first release must be done by hand. Best-effort and never a gate: a status that cannot be
    # confirmed degrades to a warning and continues. The skill treats a never-published crate in
    # the increment set as a stop.
    [CmdletBinding()]
    param(
        [string] $ManifestPath
    )

    foreach ($crate in @(Get-PublishableCrate -ManifestPath $ManifestPath)) {
        Write-Verbose "Checking crates.io publish status for '$($crate.Name)'"
        switch (Get-CratePublishStatus -Name $crate.Name) {
            'Published' { }
            'NeverPublished' {
                Write-Warning "$($crate.Name) has never been published. Its first release must be done manually (cargo publish); afterwards configure Trusted Publishing for it on crates.io and re-publish via the GitHub workflow."
            }
            default {
                Write-Warning "Could not confirm crates.io publish status for '$($crate.Name)'; skipping its never-published preflight. Verify manually if it is a brand-new crate."
            }
        }
    }
}

function Set-GitHubOutput {
    # Emits a `name=value` step output for the workflow (and echoes it for the run log). No-ops
    # the file append when GITHUB_OUTPUT is unset, so the recipes are runnable locally.
    # Empty values are opt-in because most workflow outputs, including release-asset outputs, are
    # contracts whose absence must not be hidden behind a syntactically present output line.
    [CmdletBinding(SupportsShouldProcess)]
    param(
        [Parameter(Mandatory)][string] $Name,
        [Parameter(Mandatory)][AllowEmptyString()][string] $Value,
        [switch] $AllowEmptyValue
    )

    if ($Value.Length -eq 0 -and -not $AllowEmptyValue) {
        throw "GitHub output '$Name' must not be empty."
    }

    Write-Host "$Name=$Value"
    if ($env:GITHUB_OUTPUT -and $PSCmdlet.ShouldProcess($env:GITHUB_OUTPUT, "append output '$Name'")) {
        Add-Content -Path $env:GITHUB_OUTPUT -Value "$Name=$Value" -Encoding utf8
    }
}

Export-ModuleMember -Function `
    Get-ReleaseTarget, `
    Get-DeclaredReleaseTarget, `
    Get-BinaryTarget, `
    Get-TrackedWorkspaceMember, `
    Get-PublishableBinaryCrate, `
    Get-PublishableCrate, `
    Get-CrateIndexPath, `
    Get-CratePublishStatus, `
    Test-NeverPublishedCrate, `
    Get-BinaryReleaseAsset, `
    Invoke-ReleasePublish, `
    Set-GitHubOutput
