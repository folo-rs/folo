// Azure resources backing the nightly `cargo-bench-history` benchmark-history
// data store (the `bench-history` GitHub workflow and local collection runs).
//
// Deploy this at resource-group scope (the wrapper `deploy.ps1` creates the
// resource group first, then runs `az deployment group create`). It provisions:
//
//   * a Storage account (Entra-only: shared-key access disabled, HTTPS only) that
//     holds the real, long-lived benchmark history;
//   * `Storage Blob Data Contributor` role assignments on the account for the
//     existing CI managed identity (reused, not created here) and (optionally) a
//     local developer principal.
//
// Unlike `infra/azure-bench-history/` (which provisions the *test* account and the
// shared CI managed identity together), this template does NOT create an identity
// or federated credentials. It reuses the CI identity created by that template —
// whose `main`-branch federated credential already matches the scheduled-run OIDC
// subject `repo:folo-rs/folo:ref:refs/heads/main` — and only grants it data access
// to this account. The deploy script resolves that identity's principal id and
// passes it in via `ciPrincipalId`, so this account can still be torn down and
// re-created at will (the identity it depends on lives in the other resource group).

@description('Location for all resources. Defaults to the resource group location.')
param location string = resourceGroup().location

@description('Globally-unique Storage account name (3-24 lowercase alphanumerics).')
@minLength(3)
@maxLength(24)
param storageAccountName string

@description('Principal (object) id of the existing CI managed identity to grant data access (reused from infra/azure-bench-history).')
param ciPrincipalId string

@description('Object id of a local developer principal (user or group) to grant data access. Empty skips the grant.')
param localPrincipalId string = ''

@description('Type of the local developer principal.')
@allowed([
  'User'
  'Group'
])
param localPrincipalType string = 'User'

// `Storage Blob Data Contributor`: read/write/delete blobs AND create/delete
// containers via the data plane, so the tool's `run` (which creates the
// container) and any later `prune` both work with this single role. This is the
// least-privilege role for the workload — Data Owner additionally grants POSIX
// ACL/ownership management that a flat blob container never needs.
var blobDataContributorRoleId = 'ba92f5b4-2d11-453d-a403-e96b0029c9fe'

resource storageAccount 'Microsoft.Storage/storageAccounts@2023-05-01' = {
  name: storageAccountName
  location: location
  sku: {
    name: 'Standard_LRS'
  }
  kind: 'StorageV2'
  properties: {
    accessTier: 'Hot'
    allowBlobPublicAccess: false
    // Entra-only: no account keys means no shared key to leak. The tool
    // authenticates exclusively through Microsoft Entra ID.
    allowSharedKeyAccess: false
    minimumTlsVersion: 'TLS1_2'
    supportsHttpsTrafficOnly: true
  }
}

// Disable container/blob soft delete, matching the test account: the history is
// reconstructible (`backfill` can re-bench any commit) and `prune` deletions are
// deliberate, so a soft-deleted remnant would only complicate listings and reuse.
resource blobService 'Microsoft.Storage/storageAccounts/blobServices@2023-05-01' = {
  parent: storageAccount
  name: 'default'
  properties: {
    containerDeleteRetentionPolicy: {
      enabled: false
    }
    deleteRetentionPolicy: {
      enabled: false
    }
  }
}

resource ciPrincipalBlobRole 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  name: guid(storageAccount.id, ciPrincipalId, blobDataContributorRoleId)
  scope: storageAccount
  properties: {
    roleDefinitionId: subscriptionResourceId('Microsoft.Authorization/roleDefinitions', blobDataContributorRoleId)
    principalId: ciPrincipalId
    principalType: 'ServicePrincipal'
  }
}

resource localPrincipalBlobRole 'Microsoft.Authorization/roleAssignments@2022-04-01' = if (!empty(localPrincipalId)) {
  name: guid(storageAccount.id, localPrincipalId, blobDataContributorRoleId)
  scope: storageAccount
  properties: {
    roleDefinitionId: subscriptionResourceId('Microsoft.Authorization/roleDefinitions', blobDataContributorRoleId)
    principalId: localPrincipalId
    principalType: localPrincipalType
  }
}

@description('Storage account name (record as BENCH_HISTORY_DATA_AZURE_ACCOUNT in constants.env).')
output storageAccountName string = storageAccount.name

@description('Blob service endpoint (https://<account>.blob.core.windows.net/).')
output blobEndpoint string = storageAccount.properties.primaryEndpoints.blob

@description('Subscription id (the same AZURE_SUBSCRIPTION_ID as the CI identity).')
output subscriptionId string = subscription().subscriptionId
