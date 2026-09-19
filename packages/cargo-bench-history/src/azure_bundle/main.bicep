// History provisioning, called by deploy.ps1. Existing storage resources
// are references, not PUTs: identity provisioning must not reset storage configuration.
// Bootstrap modules run only for resources the wrapper confirms are missing.
// Ref: README.md, "Deployment behavior".

@description('Location for all resources. Defaults to the resource group location.')
param location string = resourceGroup().location

@description('Globally-unique Storage account name (3-24 lowercase alphanumerics).')
@minLength(3)
@maxLength(24)
param storageAccountName string

@description('Name of the user-assigned managed identity shared by history workflows.')
param managedIdentityName string = 'id-${storageAccountName}-bench-history'

@description('History container, matching [storage.azure].container in .cargo/bench_history.toml.')
@minLength(3)
@maxLength(63)
param historyContainerName string = 'bench-history'

@description('Set true only for an absent storage account; the PowerShell driver determines this value.')
param createStorageAccount bool = false

@description('Set true only for an absent history container; the PowerShell driver determines this value.')
param createHistoryContainer bool = false

@description('GitHub organisation (or user) that owns the repository.')
param githubOrg string

@description('GitHub repository name.')
param githubRepo string

@description('History branch whose workflow runs may federate into Azure.')
@minLength(1)
param historyBranch string

@description('Object ID of an existing additional Entra user or group to grant data access. Empty skips the grant.')
param customPrincipalId string = ''

@description('Type of the custom principal.')
@allowed([
  'User'
  'Group'
])
param customPrincipalType string = 'User'

// `Storage Blob Data Contributor`: read/write/delete blobs AND create/delete
// containers via the data plane, so the tool's `run` (which creates the
// container) and any later `prune` both work with this single role. This is the
// least-privilege role for the workload — Data Owner additionally grants POSIX
// ACL/ownership management that a flat blob container never needs.
var blobDataContributorRoleId = 'ba92f5b4-2d11-453d-a403-e96b0029c9fe'

// GitHub's OIDC issuer and the audience Azure expects for the token exchange.
var githubIssuer = 'https://token.actions.githubusercontent.com'
var federationAudience = 'api://AzureADTokenExchange'

// GitHub workflow policy must restrict PR use to same-repository heads: the PR
// subject alone does not distinguish a fork head from a same-repository head.
var credentials = [
  {
    // This stable resource key is independent of the configured history branch.
    // Branch selection changes only the credential subject.
    name: 'github-branch-main'
    subject: 'repo:${githubOrg}/${githubRepo}:ref:refs/heads/${historyBranch}'
  }
  {
    name: 'github-pull-request'
    subject: 'repo:${githubOrg}/${githubRepo}:pull_request'
  }
]

module storageBootstrap './storage-bootstrap.bicep' = if (createStorageAccount) {
  name: '${deployment().name}-storage'
  params: {
    storageAccountName: storageAccountName
    location: location
  }
}

resource storageAccount 'Microsoft.Storage/storageAccounts@2025-01-01' existing = {
  name: storageAccountName
}

module containerBootstrap './container-bootstrap.bicep' = if (createHistoryContainer) {
  name: '${deployment().name}-container'
  params: {
    storageAccountName: storageAccountName
    historyContainerName: historyContainerName
  }
  dependsOn: [
    storageBootstrap
  ]
}

resource managedIdentity 'Microsoft.ManagedIdentity/userAssignedIdentities@2024-11-30' = {
  name: managedIdentityName
  location: location
}

// Federated credentials on the same identity must be created sequentially.
// A Bicep resource `for` loop deploys its iterations in parallel by default, but
// Azure rejects concurrent writes to one identity's federated-credentials
// collection (they conflict). `@batchSize(1)` serialises the loop so each
// credential is created only after the previous one finishes.
@batchSize(1)
resource federation 'Microsoft.ManagedIdentity/userAssignedIdentities/federatedIdentityCredentials@2024-11-30' = [
  for credential in credentials: {
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
  dependsOn: [
    storageBootstrap
  ]
}

resource customPrincipalBlobRole 'Microsoft.Authorization/roleAssignments@2022-04-01' = if (!empty(customPrincipalId)) {
  name: guid(storageAccount.id, customPrincipalId, blobDataContributorRoleId)
  scope: storageAccount
  properties: {
    roleDefinitionId: subscriptionResourceId('Microsoft.Authorization/roleDefinitions', blobDataContributorRoleId)
    principalId: customPrincipalId
    principalType: customPrincipalType
  }
  dependsOn: [
    storageBootstrap
  ]
}

@description('Storage account name (record as `account` in .cargo/bench_history.toml).')
output storageAccountName string = storageAccount.name

@description('History container name (record as `container` in .cargo/bench_history.toml).')
output historyContainerName string = historyContainerName

@description('Blob service endpoint (https://<account>.blob.core.windows.net/).')
output blobEndpoint string = storageAccount.properties.primaryEndpoints.blob

@description('Non-secret client id of the managed identity for workflow OIDC login.')
output managedIdentityClientId string = managedIdentity.properties.clientId

@description('Principal (object) id of the managed identity.')
output managedIdentityPrincipalId string = managedIdentity.properties.principalId

@description('Entra tenant id for workflow OIDC login.')
output tenantId string = subscription().tenantId

@description('Subscription id for workflow OIDC login.')
output subscriptionId string = subscription().subscriptionId
