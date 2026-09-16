// Called by main.bicep only for a missing history container, before its reader
// role assignment. Management-plane provisioning needs no storage account keys.
param storageAccountName string
param historyContainerName string

resource storageAccount 'Microsoft.Storage/storageAccounts@2023-05-01' existing = {
  name: storageAccountName
}

resource blobService 'Microsoft.Storage/storageAccounts/blobServices@2023-05-01' existing = {
  parent: storageAccount
  name: 'default'
}

resource historyContainer 'Microsoft.Storage/storageAccounts/blobServices/containers@2023-05-01' = {
  parent: blobService
  name: historyContainerName
  properties: {
    publicAccess: 'None'
  }
}
