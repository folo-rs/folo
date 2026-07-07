#requires -Version 7

# Azure OIDC federation identity plumbing for the benchmark-history workflow
# (.github/workflows/bench-history.yml).
#
# The `collect` and `analyze` jobs both federate into Azure with the SAME two identifiers, which
# live (non-secret) in constants.env. Two jobs re-exporting those under the standard AZURE_* names
# is exactly the kind of copy-pasted inline snippet that drifts - and where the recent
# case-collision footgun hid - so the parse-and-map logic lives here, behind small seams the Pester
# suite (AzureFederation.Tests.ps1) exercises, and each job's workflow step is a thin import + call.
#
# constants.env is read directly (not via `just`'s dotenv) because these steps re-export under
# different names (AZURE_PROD_CLIENT_ID -> AZURE_CLIENT_ID) and must fail loudly when an identifier
# is missing rather than federate later with an empty value and an opaque error.

Set-StrictMode -Version Latest

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
        throw "constants.env is missing a non-empty '$Name' (required for Azure federation)."
    }
    return $value
}

function Set-AzureFederationEnv {
    # Reads the two federation identifiers from $ConstantsPath and appends them, under the standard
    # AZURE_* names the tool and azure/login read, to the GitHub step-env file at $EnvFilePath
    # (normally $env:GITHUB_ENV). The mapping is deliberate: constants.env names them
    # AZURE_PROD_CLIENT_ID / AZURE_TENANT_ID, the consumers expect AZURE_CLIENT_ID / AZURE_TENANT_ID.
    # Returns the mapping it wrote so a caller (and the tests) can inspect it.
    [CmdletBinding(SupportsShouldProcess)]
    [OutputType([System.Collections.Specialized.OrderedDictionary])]
    param(
        [Parameter(Mandatory)][string] $ConstantsPath,
        [Parameter(Mandatory)][string] $EnvFilePath
    )

    $constants = Read-DotEnvFile -Path $ConstantsPath
    $exported = [ordered]@{
        AZURE_CLIENT_ID = Get-RequiredConstant -Values $constants -Name 'AZURE_PROD_CLIENT_ID'
        AZURE_TENANT_ID = Get-RequiredConstant -Values $constants -Name 'AZURE_TENANT_ID'
    }

    $lines = foreach ($entry in $exported.GetEnumerator()) { "$($entry.Key)=$($entry.Value)" }
    if ($PSCmdlet.ShouldProcess($EnvFilePath, 'Append AZURE_CLIENT_ID and AZURE_TENANT_ID')) {
        $lines | Add-Content -Path $EnvFilePath -Encoding utf8
    }
    Write-Verbose "Exported AZURE_CLIENT_ID and AZURE_TENANT_ID from '$ConstantsPath' to '$EnvFilePath' so the collect/analyze jobs federate into Azure with the same committed, non-secret identifiers."
    return $exported
}

Export-ModuleMember -Function Read-DotEnvFile, Get-RequiredConstant, Set-AzureFederationEnv
