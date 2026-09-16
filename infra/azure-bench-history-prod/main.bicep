// Production history provisioning, called by deploy.ps1. Existing storage resources
// are references, not PUTs: adding reader access must not reset storage configuration.
// Bootstrap modules run only for resources the wrapper confirms are missing.
// Ref: README.md, "Deployment behavior" and "Retire the writer's PR credential".

@description('Location for all resources. Defaults to the resource group location.')
param location string = resourceGroup().location

@description('Globally-unique Storage account name (3-24 lowercase alphanumerics).')
@minLength(3)
@maxLength(24)
param storageAccountName string

@description('Name of the user-assigned managed identity used by the nightly workflow.')
param managedIdentityName string = 'id-folo-bench-history-prod'

@description('Name of the dedicated read-only production history identity.')
param readerManagedIdentityName string = '${managedIdentityName}-reader'

@description('History container, matching [storage.azure].container in .cargo/bench_history.toml.')
@minLength(3)
@maxLength(63)
param historyContainerName string = 'bench-history'

@description('Create the storage account and initial blob-service settings only when the account is missing.')
param createStorageAccount bool = false

@description('Create the history container only when missing, before assigning its reader role.')
param createHistoryContainer bool = false

@description('GitHub organisation (or user) that owns the repository.')
param githubOrg string = 'folo-rs'

@description('GitHub repository name.')
param githubRepo string = 'folo'

@description('Branches whose workflow runs may federate into Azure (one federated credential each).')
param githubBranches array = [
  'main'
]

@description('Whether to provision writer PR trust. False omits creation; it does not delete an existing credential. deploy.ps1 preserves observed trust unless retirement is explicitly requested.')
param trustPullRequests bool = false

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

// `Storage Blob Data Reader`: read/list history without changing containers or blobs.
var blobDataReaderRoleId = '2a2b9908-6ea1-4ae2-8e65-a410df84e7d1'

// GitHub's OIDC issuer and the audience Azure expects for the token exchange.
var githubIssuer = 'https://token.actions.githubusercontent.com'
var federationAudience = 'api://AzureADTokenExchange'

// The reader trusts main and PR subjects independently of staged writer retirement.
// GitHub workflow policy must restrict PR use to same-repository heads: the PR
// subject alone does not distinguish a fork head from a same-repository head.
var branchCredentials = [
  for branch in githubBranches: {
    name: 'github-branch-${replace(branch, '/', '-')}'
    subject: 'repo:${githubOrg}/${githubRepo}:ref:refs/heads/${branch}'
  }
]
var pullRequestCredentials = [
  {
    name: 'github-pull-request'
    subject: 'repo:${githubOrg}/${githubRepo}:pull_request'
  }
]
var writerCredentials = concat(branchCredentials, trustPullRequests ? pullRequestCredentials : [])
var readerCredentials = concat(branchCredentials, pullRequestCredentials)

module storageBootstrap './storage-bootstrap.bicep' = if (createStorageAccount) {
  name: '${deployment().name}-storage'
  params: {
    storageAccountName: storageAccountName
    location: location
  }
}

resource storageAccount 'Microsoft.Storage/storageAccounts@2023-05-01' existing = {
  name: storageAccountName
}

resource blobService 'Microsoft.Storage/storageAccounts/blobServices@2023-05-01' existing = {
  parent: storageAccount
  name: 'default'
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

resource historyContainer 'Microsoft.Storage/storageAccounts/blobServices/containers@2023-05-01' existing = {
  parent: blobService
  name: historyContainerName
}

resource managedIdentity 'Microsoft.ManagedIdentity/userAssignedIdentities@2023-01-31' = {
  name: managedIdentityName
  location: location
}

resource readerManagedIdentity 'Microsoft.ManagedIdentity/userAssignedIdentities@2023-01-31' = {
  name: readerManagedIdentityName
  location: location
}

// Federated credentials on the same identity must be created sequentially.
// A Bicep resource `for` loop deploys its iterations in parallel by default, but
// Azure rejects concurrent writes to one identity's federated-credentials
// collection (they conflict). `@batchSize(1)` serialises the loop so each
// credential is created only after the previous one finishes.
@batchSize(1)
resource federation 'Microsoft.ManagedIdentity/userAssignedIdentities/federatedIdentityCredentials@2023-01-31' = [
  for credential in writerCredentials: {
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

@batchSize(1)
resource readerFederation 'Microsoft.ManagedIdentity/userAssignedIdentities/federatedIdentityCredentials@2023-01-31' = [
  for credential in readerCredentials: {
    parent: readerManagedIdentity
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

resource readerManagedIdentityBlobRole 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  name: guid(historyContainer.id, readerManagedIdentity.id, blobDataReaderRoleId)
  scope: historyContainer
  properties: {
    roleDefinitionId: subscriptionResourceId('Microsoft.Authorization/roleDefinitions', blobDataReaderRoleId)
    principalId: readerManagedIdentity.properties.principalId
    principalType: 'ServicePrincipal'
  }
  dependsOn: [
    containerBootstrap
  ]
}

resource localPrincipalBlobRole 'Microsoft.Authorization/roleAssignments@2022-04-01' = if (!empty(localPrincipalId)) {
  name: guid(storageAccount.id, localPrincipalId, blobDataContributorRoleId)
  scope: storageAccount
  properties: {
    roleDefinitionId: subscriptionResourceId('Microsoft.Authorization/roleDefinitions', blobDataContributorRoleId)
    principalId: localPrincipalId
    principalType: localPrincipalType
  }
  dependsOn: [
    storageBootstrap
  ]
}

@description('Storage account name (record as `account` in .cargo/bench_history.toml).')
output storageAccountName string = storageAccount.name

@description('History container name (record as `container` in .cargo/bench_history.toml).')
output historyContainerName string = historyContainer.name

@description('Blob service endpoint (https://<account>.blob.core.windows.net/).')
output blobEndpoint string = storageAccount.properties.primaryEndpoints.blob

@description('Client id of the managed identity (record as AZURE_PROD_CLIENT_ID in constants.env).')
output managedIdentityClientId string = managedIdentity.properties.clientId

@description('Principal (object) id of the managed identity.')
output managedIdentityPrincipalId string = managedIdentity.properties.principalId

@description('Non-secret reader client id (record as AZURE_PROD_READER_CLIENT_ID in constants.env).')
output readerManagedIdentityClientId string = readerManagedIdentity.properties.clientId

@description('Principal (object) id of the production history reader identity.')
output readerManagedIdentityPrincipalId string = readerManagedIdentity.properties.principalId

@description('Entra tenant id (the same AZURE_TENANT_ID as the test identity).')
output tenantId string = subscription().tenantId

@description('Subscription id (the same AZURE_SUBSCRIPTION_ID as the test identity).')
output subscriptionId string = subscription().subscriptionId
