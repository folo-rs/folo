#Requires -Version 7.6

# Compatibility import for repository Pester callers. The published shell owns
# the one Azure provisioning policy, also used by exported standalone bundles.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

Import-Module (Join-Path $PSScriptRoot '..' '..' 'packages' 'cargo-bench-history' 'src' 'azure_bundle' 'ProductionIdentityDeployment.psm1') -Force
Export-ModuleMember -Function Invoke-ProductionIdentityDeployment
