#requires -Version 7

# Reads the existing non-secret test identity configuration for the hosted benchmark caller
# canary before Rust/bootstrap setup. Production callers use repository variables directly.
# Missing values fail here rather than causing an opaque federation error in another job.
# Ref: .github/workflows/implementation.md#reusable-workflow-canary.

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

function Read-DotEnvFile {
    # Parses a KEY=value dotenv file (constants.env) into an ordered hashtable. Blank lines and
    # `#` comment lines are skipped; keys and values are trimmed. A later duplicate key wins, matching
    # dotenv semantics. Isolated so the tests can feed a fixture file without touching the real one.
    [CmdletBinding()]
    [OutputType([System.Collections.Specialized.OrderedDictionary])]
    param([Parameter(Mandatory)][string] $Path)

    if (-not (Test-Path -LiteralPath $Path)) {
        throw "Dotenv file '$Path' does not exist."
    }

    $values = [ordered]@{}
    foreach ($line in Get-Content -LiteralPath $Path) {
        # KEY=value, where KEY has no '#' or '=' (so a leading-# comment line never matches). The
        # value is everything after the first '=', so values may themselves contain '='.
        if ($line -match '^\s*([^#=]+)=(.*)$') {
            $values[$Matches[1].Trim()] = $Matches[2].Trim()
        }
    }
    return $values
}

function Get-RequiredConstant {
    # Returns $Values[$Name], throwing when it is absent or blank. Federation with an empty
    # AZURE_CLIENT_ID / AZURE_TENANT_ID would otherwise fail far later with an opaque error; failing
    # here names the exact missing constant. Accessing a missing hashtable key returns $null under
    # strict mode (it does not throw), so the explicit blank check is what catches an absent key.
    [CmdletBinding()]
    [OutputType([string])]
    param(
        [Parameter(Mandatory)][System.Collections.IDictionary] $Values,
        [Parameter(Mandatory)][string] $Name
    )

    $value = $Values[$Name]
    if ([string]::IsNullOrWhiteSpace($value)) {
        throw "constants.env is missing a non-empty '$Name'."
    }
    return $value
}

Export-ModuleMember -Function Read-DotEnvFile, Get-RequiredConstant
