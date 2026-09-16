// Optional direct-ARM parameter file. deploy.ps1 supplies equivalent CLI parameters
// after observing existing resources and writer trust. Direct callers must supply
// the lifecycle decisions explicitly; false trust omits creation, not deletion.
using './main.bicep'

param storageAccountName = readEnvironmentVariable('AZURE_STORAGE_ACCOUNT_NAME')
param location = readEnvironmentVariable('AZURE_LOCATION', 'swedencentral')
param managedIdentityName = readEnvironmentVariable('AZURE_MANAGED_IDENTITY_NAME', 'id-folo-bench-history-prod')
param readerManagedIdentityName = readEnvironmentVariable('AZURE_READER_MANAGED_IDENTITY_NAME', '${managedIdentityName}-reader')
param historyContainerName = readEnvironmentVariable('AZURE_HISTORY_CONTAINER_NAME', 'bench-history')
param createStorageAccount = bool(readEnvironmentVariable('AZURE_CREATE_STORAGE_ACCOUNT'))
param createHistoryContainer = bool(readEnvironmentVariable('AZURE_CREATE_HISTORY_CONTAINER'))
param trustPullRequests = bool(readEnvironmentVariable('AZURE_TRUST_PULL_REQUESTS'))
param githubOrg = readEnvironmentVariable('GITHUB_ORG', 'folo-rs')
param githubRepo = readEnvironmentVariable('GITHUB_REPO', 'folo')
param localPrincipalId = readEnvironmentVariable('AZURE_LOCAL_PRINCIPAL_ID', '')
param localPrincipalType = readEnvironmentVariable('AZURE_LOCAL_PRINCIPAL_TYPE', 'User')
