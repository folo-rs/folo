// Parameters for main.bicep, sourced from environment variables so `deploy.ps1`
// can supply them. Run via: az deployment group create -f main.bicep -p main.bicepparam
using './main.bicep'

param storageAccountName = readEnvironmentVariable('AZURE_STORAGE_ACCOUNT_NAME')
param location = readEnvironmentVariable('AZURE_LOCATION', 'swedencentral')
param ciPrincipalId = readEnvironmentVariable('AZURE_CI_PRINCIPAL_ID')
param localPrincipalId = readEnvironmentVariable('AZURE_LOCAL_PRINCIPAL_ID', '')
param localPrincipalType = readEnvironmentVariable('AZURE_LOCAL_PRINCIPAL_TYPE', 'User')
