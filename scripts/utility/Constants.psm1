#requires -Version 7.6

# Reads repository constants before Just's dotenv loading is available. Used by tool
# bootstrapping and the hosted benchmark caller canary, both before Rust helper preparation.
# Ref: docs/build-and-tooling.md#development-tool-installation.

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
    # Fail at configuration loading rather than passing an empty pin or identity to a subprocess.
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
