#requires -Version 7.6

# Bootstraps the pinned Just command runner for DEVELOPMENT.md and setup-environment.
# This entry point cannot itself be a Just recipe. Both callers share CargoTools' verified
# binstall bootstrap and publisher-first installation policy, without changing machine settings.
[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$VerbosePreference = 'Continue'

Import-Module (Join-Path $PSScriptRoot 'CargoTools.psm1') -Force
$version = Get-BootstrapToolVersion -Name JUST
Install-CargoTool -Package "just@$version"
