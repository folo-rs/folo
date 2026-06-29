// Azure resources backing the `cargo-bench-history` real-Azure storage tests.
//
// Deploy this at resource-group scope (the wrapper `deploy.ps1` creates the
// resource group first, then runs `az deployment group create`). It provisions:
//
//   * a Storage account (Entra-only: shared-key access disabled, HTTPS only);
//   * a user-assigned managed identity (the CI principal) with GitHub OIDC
//     federated credentials, so GitHub Actions can sign in without a stored
//     secret;
//   * `Storage Blob Data Contributor` role assignments on the account for the
//     managed identity and (optionally) a local developer principal.
//
// Everything is idempotent and fully described here, so the resource group can be
// torn down (`teardown.ps1`) and re-created at will.

@description('Location for all resources. Defaults to the resource group location.')
param location string = resourceGroup().location

@description('Globally-unique Storage account name (3-24 lowercase alphanumerics).')
@minLength(3)
@maxLength(24)
param storageAccountName string

@description('Name of the user-assigned managed identity used by CI (GitHub Actions).')
param managedIdentityName string = 'id-folo-bench-history-ci'

@description('GitHub organisation (or user) that owns the repository.')
param githubOrg string = 'folo-rs'

@description('GitHub repository name.')
param githubRepo string = 'folo'

@description('Branches whose workflow runs may federate into Azure (one federated credential each).')
param githubBranches array = [
  'main'
]

@description('Whether to trust pull-request workflow runs from the same repository.')
param trustPullRequests bool = true

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
// container) and the tests' container cleanup both work with this single role.
// Data Owner additionally grants POSIX ACL/ownership management that a flat blob
// container never needs, so Contributor is the least-privilege fit (matching the
// prod stack).
var blobDataContributorRoleId = 'ba92f5b4-2d11-453d-a403-e96b0029c9fe'

// GitHub's OIDC issuer and the audience Azure expects for the token exchange.
var githubIssuer = 'https://token.actions.githubusercontent.com'
var federationAudience = 'api://AzureADTokenExchange'

// One federated credential per trusted branch, plus an optional pull-request one.
var branchCredentials = [
  for branch in githubBranches: {
    name: 'github-branch-${replace(branch, '/', '-')}'
    subject: 'repo:${githubOrg}/${githubRepo}:ref:refs/heads/${branch}'
  }
]
var pullRequestCredential = trustPullRequests
  ? [
      {
        name: 'github-pull-request'
        subject: 'repo:${githubOrg}/${githubRepo}:pull_request'
      }
    ]
  : []
var federatedCredentials = concat(branchCredentials, pullRequestCredential)

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
    // Entra-only: no account keys means no shared key to leak. The tests and the
    // tool authenticate exclusively through Microsoft Entra ID.
    allowSharedKeyAccess: false
    minimumTlsVersion: 'TLS1_2'
    supportsHttpsTrafficOnly: true
  }
}

// Disable container/blob soft delete so a deleted test container is gone
// immediately and its name can be reused without colliding with a soft-deleted
// remnant.
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

resource managedIdentity 'Microsoft.ManagedIdentity/userAssignedIdentities@2023-01-31' = {
  name: managedIdentityName
  location: location
}

// Federated credentials on the same identity must be created sequentially.
// A Bicep resource `for` loop deploys its iterations in parallel by default, but
// Azure rejects concurrent writes to one identity's federated-credentials
// collection (they conflict). `@batchSize(1)` serialises the loop so each
// credential is created only after the previous one finishes.
@batchSize(1)
resource federation 'Microsoft.ManagedIdentity/userAssignedIdentities/federatedIdentityCredentials@2023-01-31' = [
  for credential in federatedCredentials: {
    parent: managedIdentity
    name: credential.name
    properties: {
      issuer: githubIssuer
      subject: credential.subject
      audiences: [
        federationAudience
      ]
    }
  }
]

resource managedIdentityBlobRole 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  name: guid(storageAccount.id, managedIdentity.id, blobDataContributorRoleId)
  scope: storageAccount
  properties: {
    roleDefinitionId: subscriptionResourceId('Microsoft.Authorization/roleDefinitions', blobDataContributorRoleId)
    principalId: managedIdentity.properties.principalId
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

@description('Storage account name (record as BENCH_HISTORY_TEST_AZURE_ACCOUNT in constants.env).')
output storageAccountName string = storageAccount.name

@description('Blob service endpoint (https://<account>.blob.core.windows.net/).')
output blobEndpoint string = storageAccount.properties.primaryEndpoints.blob

@description('Client id of the managed identity (record as AZURE_TEST_CLIENT_ID in constants.env).')
output managedIdentityClientId string = managedIdentity.properties.clientId

@description('Principal (object) id of the managed identity.')
output managedIdentityPrincipalId string = managedIdentity.properties.principalId

@description('Entra tenant id (record as AZURE_TENANT_ID in constants.env).')
output tenantId string = subscription().tenantId

@description('Subscription id (record as AZURE_SUBSCRIPTION_ID in constants.env).')
output subscriptionId string = subscription().subscriptionId
