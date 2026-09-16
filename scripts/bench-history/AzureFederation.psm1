#requires -Version 7

# Azure OIDC federation identity plumbing for the benchmark-history workflow
# (.github/workflows/bench-history.yml).
#
# Collection/backfill use the writer identity; analysis uses the separately scoped reader.
# Their non-secret client IDs and shared tenant live in constants.env. This module keeps the
# required-value validation and standard AZURE_* export consistent across workflow jobs.
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
    # Selects the requested capability without a reader-to-writer fallback. Writer is the default
    # for existing collection/backfill callers; analysis explicitly requests Reader. A source-built
    # preparation job may also call this to validate identifiers without holding id-token permission.
    [CmdletBinding(SupportsShouldProcess)]
    [OutputType([System.Collections.Specialized.OrderedDictionary])]
    param(
        [Parameter(Mandatory)][string] $ConstantsPath,
        [Parameter(Mandatory)][string] $EnvFilePath,
        [ValidateSet('Writer', 'Reader')][string] $Access = 'Writer'
    )

    $constants = Read-DotEnvFile -Path $ConstantsPath
    $clientConstant = if ($Access -eq 'Reader') { 'AZURE_PROD_READER_CLIENT_ID' } else { 'AZURE_PROD_CLIENT_ID' }
    $clientId = Get-RequiredConstant -Values $constants -Name $clientConstant
    if ($Access -eq 'Reader' -and $clientId -eq $constants['AZURE_PROD_CLIENT_ID']) {
        throw 'The production reader must use its own identity, not AZURE_PROD_CLIENT_ID. Deploy the reader identity before activating analysis.'
    }
    $exported = [ordered]@{
        AZURE_CLIENT_ID = $clientId
        AZURE_TENANT_ID = Get-RequiredConstant -Values $constants -Name 'AZURE_TENANT_ID'
    }

    $lines = foreach ($entry in $exported.GetEnumerator()) { "$($entry.Key)=$($entry.Value)" }
    if ($PSCmdlet.ShouldProcess($EnvFilePath, 'Append AZURE_CLIENT_ID and AZURE_TENANT_ID')) {
        $lines | Add-Content -Path $EnvFilePath -Encoding utf8
    }
    Write-Verbose "Selected $Access access from $clientConstant in '$ConstantsPath'; exported its client ID and the tenant to '$EnvFilePath'."
    return $exported
}

Export-ModuleMember -Function Read-DotEnvFile, Get-RequiredConstant, Set-AzureFederationEnv
