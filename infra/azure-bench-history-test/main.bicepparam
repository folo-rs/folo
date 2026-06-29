// Parameters for main.bicep, sourced from environment variables so `deploy.ps1`
// can supply them. Run via: az deployment group create -f main.bicep -p main.bicepparam
using './main.bicep'

param storageAccountName = readEnvironmentVariable('AZURE_STORAGE_ACCOUNT_NAME')
param location = readEnvironmentVariable('AZURE_LOCATION', 'swedencentral')
param managedIdentityName = readEnvironmentVariable('AZURE_MANAGED_IDENTITY_NAME', 'id-folo-bench-history-ci')
param githubOrg = readEnvironmentVariable('GITHUB_ORG', 'folo-rs')
param githubRepo = readEnvironmentVariable('GITHUB_REPO', 'folo')
param localPrincipalId = readEnvironmentVariable('AZURE_LOCAL_PRINCIPAL_ID', '')
param localPrincipalType = readEnvironmentVariable('AZURE_LOCAL_PRINCIPAL_TYPE', 'User')
