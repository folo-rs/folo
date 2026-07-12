#requires -Version 7

# Machine-key threading for the benchmark-history `analyze` step, shared by the push-to-main workflow
# (.github/workflows/bench-history.yml, via the gh-analyze-bench-history recipe) and the per-PR
# workflow (.github/workflows/pr-bench-history.yml, via gh-analyze-pr-bench-history).
#
# Collection runs as a matrix across a heterogeneous GitHub runner pool, so each leg stamps its
# results with its OWN real hardware fingerprint (there is no longer a fixed `github` key). Analysis,
# by contrast, runs as a single job that cannot re-derive those keys from its own hardware, so each
# collect leg writes its fingerprint to a file and uploads it as an artifact; the analyze job
# downloads them all into one directory and this module turns that directory into the repeated
# `--machine-key <fingerprint>` argument vector the tool is invoked with. Threading the EXACT keys
# collected this run (rather than `--machine-key all`) keeps the analysis scoped to the machines that
# actually measured this commit, without assuming anything about other data in the shared store.
#
# Building the vector is real logic - directory scan, per-file validation, dedupe and ordering, plus
# the empty-directory edge case a total collect failure produces - so it lives here behind a seam the
# Pester suite (BenchHistoryMachineKey.Tests.ps1) exercises, and the recipe is a thin import + call.

Set-StrictMode -Version Latest

function Get-MachineKeyArgument {
    # Reads every machine-key file the collect matrix uploaded into $KeyDirectory and returns the
    # `--machine-key <fingerprint>` argument vector (a string[]) to splat into the analyze tool call.
    # Each collect leg's artifact contributes one file holding a single 16-hex-character fingerprint
    # (as emitted by `cargo-bench-history machine-key`); files are read recursively so it does not
    # matter whether the download flattened them or kept one subdirectory per artifact.
    #
    # Keys are trimmed, lowercased, de-duplicated and sorted so two runners with identical hardware
    # collapse to one `--machine-key` (the tool would otherwise see a redundant repeat) and the
    # argument order is deterministic for stable logs. A file that is missing, empty, or does not hold
    # a valid fingerprint is a corrupted upload rather than a benign state, so it throws rather than
    # silently narrowing the analysis.
    #
    # An absent or empty directory returns an empty vector, NOT an error: a total collect failure
    # (every matrix leg failed, so nothing was uploaded) legitimately yields zero keys, and the caller
    # detects that and skips the analysis (there is no new data to survey) while the workflow's
    # separate collect-failure alert does the notifying. Only hardware-DEPENDENT engines partition by
    # machine key; deterministic engines (Callgrind, alloc_tracker) are exempt from the machine-key
    # filter inside the tool, so a non-empty vector still analyzes them too.
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
            'keys (a total collect failure uploads nothing). The caller skips analysis.')
        return @()
    }

    $files = @(Get-ChildItem -LiteralPath $KeyDirectory -Recurse -File)
    if ($files.Count -eq 0) {
        Write-Verbose ("Machine-key directory '$KeyDirectory' is empty: zero collected keys. The " +
            'caller skips analysis.')
        return @()
    }

    $keys = [System.Collections.Generic.List[string]]::new()
    foreach ($file in $files) {
        $raw = Get-Content -LiteralPath $file.FullName -Raw
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
    Write-Verbose ("Threading $($unique.Count) machine key(s) into analysis: " +
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

Export-ModuleMember -Function Get-MachineKeyArgument
