#requires -Version 7

# Builds analysis arguments for the history and PR workflows through gh-analyze-bench-history.
# The companion reconciles collection receipts and writes the selected machine-key files.
# These keys scope analysis to this workflow's collection evidence, rather than every stored
# machine partition at the commit. The analysis runner must not substitute its own fingerprint.
# Directory scanning, key validation, deduplication and argument assembly are covered by Pester.

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

function Get-MachineKeyArgument {
    # Reads the reconciled machine-key files in $KeyDirectory and returns the
    # `--machine-key <fingerprint>` argument vector (a string[]) to splat into the analyze tool call.
    # Each selected platform contributes one `machine-key.txt` holding its receipt's fingerprint
    # (as emitted by `cargo-bench-history machine-key`); the scan is restricted to that exact filename
    # and recursive, so callers may retain platform subdirectories while stray files (e.g.
    # `actions/download-artifact` metadata, a `.DS_Store`, an accidental readme) are ignored rather
    # than mistaken for a corrupt key and failing the whole analysis.
    #
    # Keys are trimmed, lowercased, de-duplicated and sorted so two runners with identical hardware
    # collapse to one `--machine-key` (the tool would otherwise see a redundant repeat) and the
    # argument order is deterministic for stable logs. A file that is missing, empty, or does not hold
    # a valid fingerprint is a corrupted upload rather than a benign state, so it throws rather than
    # silently narrowing the analysis.
    #
    # An absent or empty directory returns an empty vector, NOT an error: a total collect failure
    # (every matrix leg failed, so nothing was uploaded) legitimately yields zero keys, and the caller
    # rejects analysis without collection evidence. Every engine is machine-keyed; no engine is
    # exempt from the filter.
    [CmdletBinding()]
    [OutputType([string[]])]
    param(
        [Parameter(Mandatory)]
        [AllowEmptyString()]
        [AllowNull()]
        [string] $KeyDirectory
    )

    if ([string]::IsNullOrWhiteSpace($KeyDirectory) -or -not (Test-Path -LiteralPath $KeyDirectory)) {
        Write-Verbose ("No machine-key directory at '$KeyDirectory': treating as zero collected " +
            'keys. The analysis command requires collection evidence and rejects an empty set.')
        return @()
    }

    # Scan recursively but ONLY for the `machine-key.txt` each collect leg writes: restricting the
    # filename means an unrelated file the artifact download may leave in the tree (metadata, a
    # `.DS_Store`, a stray readme) is skipped instead of failing fingerprint validation, and
    # -ErrorAction Stop turns any enumeration error into a hard failure rather than a partial key set.
    $files = @(Get-ChildItem -LiteralPath $KeyDirectory -Recurse -File -Filter 'machine-key.txt' -ErrorAction Stop)
    if ($files.Count -eq 0) {
        Write-Verbose ("Machine-key directory '$KeyDirectory' holds no machine-key.txt files: zero " +
            'collected keys. The analysis command rejects an empty set.')
        return @()
    }

    $keys = [System.Collections.Generic.List[string]]::new()
    foreach ($file in $files) {
        # Read with a terminating error: a genuinely empty file returns $null (handled as the
        # "is empty" corrupt-upload case below), but an unreadable file (permissions/IO) must surface
        # its real failure here rather than fall through to the misleading "is empty" guard.
        $raw = Get-Content -LiteralPath $file.FullName -Raw -ErrorAction Stop
        $key = if ($null -eq $raw) { '' } else { $raw.Trim() }
        if ($key -eq '') {
            throw ("Machine-key file '$($file.FullName)' is empty. Each collect leg writes exactly " +
                'one fingerprint; an empty file means the key-writing step produced no output and ' +
                'the upload is corrupt.')
        }

        # A fingerprint is the lowercase hex of a truncated SHA-256 (cbh_probe FINGERPRINT_HEX_LEN),
        # so it is exactly 16 hex characters. Rejecting anything else fails loudly on a corrupt upload
        # instead of threading garbage into `--machine-key` (which would silently match no series).
        if ($key -notmatch '^[0-9a-fA-F]{16}$') {
            throw ("Machine-key file '$($file.FullName)' does not contain a 16-hex-character " +
                "fingerprint; got '$key'. Collection writes the key with `cargo-bench-history " +
                'machine-key`, so a malformed value indicates a corrupt upload.')
        }

        $keys.Add($key.ToLowerInvariant())
    }

    $unique = @($keys | Sort-Object -Unique)
    $noun = if ($unique.Count -eq 1) { 'machine key' } else { 'machine keys' }
    Write-Verbose ("Threading $($unique.Count) $noun into analysis: " +
        ($unique -join ', ') + '.')

    $arguments = [System.Collections.Generic.List[string]]::new()
    foreach ($key in $unique) {
        $arguments.Add('--machine-key')
        $arguments.Add($key)
    }

    # Comma-wrap the typed array so a single-key result is still returned as a [string[]] rather than
    # unrolled to a bare string by PowerShell's pipeline, keeping the contract the caller splats.
    return , $arguments.ToArray()
}

function Get-BenchHistoryAnalysisCommand {
    # Thin argument assembly shared by history and PR analysis. The Rust companion validates
    # receipts and report decisions; this wrapper supplies the existing CLI's paths and filters.
    [CmdletBinding()]
    [OutputType([string[]])]
    param(
        [Parameter(Mandatory)][string] $KeyDirectory,
        [Parameter(Mandatory)][string] $ReportDirectory,
        [Parameter(Mandatory)][string] $Context,
        [Parameter(Mandatory)][string] $Base,
        [AllowEmptyString()][string] $Repository = ''
    )
    $keys = Get-MachineKeyArgument -KeyDirectory $KeyDirectory -Verbose
    if ($null -eq $keys -or $keys.Count -eq 0) {
        throw 'Analysis requires at least one validated collection machine key; no placeholder report will be emitted.'
    }
    $arguments = @(
        'analyze', '--engine', 'all', '--target-triple', 'all'
    ) + $keys + @(
        # CI compares clean stored commits, not developer snapshots for the same branch.
        # Ref: .github/workflows/implementation.md#benchmark-workflow-artifacts.
        '--context', $Context, '--base', $Base, '--no-dirty', '--verbose'
        "--cache=$(Join-Path $ReportDirectory 'cache')"
        '--no-text'
        '--markdown', (Join-Path $ReportDirectory 'report.md')
        '--json', (Join-Path $ReportDirectory 'report.json')
        '--markdown-summary', (Join-Path $ReportDirectory 'summary.md')
    )
    if (-not [string]::IsNullOrWhiteSpace($Repository)) {
        $arguments += @('--repo', $Repository)
    }
    return , [string[]] $arguments
}

Export-ModuleMember -Function Get-MachineKeyArgument, Get-BenchHistoryAnalysisCommand
