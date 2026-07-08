#requires -Version 7

# Shared shard-spec parsing for the CI-sharded recipes (`just mutants`, `just miri-harder`).
#
# Both recipes accept the same 1-based "N/M" shard spec (so `1/8 .. 8/8` selects one of eight
# shards) and used to carry a byte-for-byte copy of the same parse-and-validate block. That
# duplication is exactly the kind of place a copy-paste bug (or a strict-mode landmine) hides
# unnoticed, so the parsing lives here once, is exercised by Pester (Sharding.Tests.ps1), and is
# consumed by both the mutants and miri-harder helper modules. What each recipe does with the
# parsed shard differs (cargo-mutants wants a 0-based `--shard`, miri-harder slices a seed range),
# so only the parsing is shared; those conversions live in the respective consumer modules.

Set-StrictMode -Version Latest

function ConvertFrom-ShardSpec {
    # Parses and validates a 1-based "N/M" shard spec, returning the shard index and count as a
    # [pscustomobject] with integer Index and Count properties (both 1-based, exactly as written).
    # Throws a descriptive terminating error for any malformed spec so the calling recipe aborts.
    # The caller is responsible for handling the "no sharding" case (an empty spec) BEFORE calling
    # this: an empty string is not a valid shard spec and is rejected here like any other garbage.
    [CmdletBinding()]
    [OutputType([pscustomobject])]
    param(
        [Parameter(Mandatory)][string] $Spec
    )

    $parts = $Spec -split '/'
    if ($parts.Length -ne 2) {
        throw "Invalid SHARD value '$Spec'. Expected format 'N/M'."
    }

    $index = 0
    $count = 0
    if (-not [int]::TryParse($parts[0], [ref] $index) -or -not [int]::TryParse($parts[1], [ref] $count)) {
        throw "Invalid SHARD value '$Spec'. N and M must be integers in 'N/M'."
    }

    if ($count -le 0) {
        throw "Invalid SHARD value '$Spec'. M must be a positive integer in 'N/M'."
    }

    if ($index -lt 1 -or $index -gt $count) {
        throw "Invalid SHARD value '$Spec'. N must satisfy 1 <= N <= M in 'N/M'."
    }

    return [pscustomobject]@{
        Index = $index
        Count = $count
    }
}

Export-ModuleMember -Function ConvertFrom-ShardSpec
