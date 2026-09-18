// Called by main.bicep only for a missing account. Existing accounts and blob
// services must not be redeployed with bootstrap defaults; see README.md.
param storageAccountName string
param location string

resource storageAccount 'Microsoft.Storage/storageAccounts@2023-05-01' = {
  name: storageAccountName
  location: location
  // Reconstructible history favors the lower cost of local redundancy; exported
  // bundles can select stronger redundancy for different durability needs.
  sku: {
    name: 'Standard_LRS'
  }
  kind: 'StorageV2'
  properties: {
    // Analysis repeatedly reads history, so use a tier intended for frequent access.
    accessTier: 'Hot'
    allowBlobPublicAccess: false
    allowSharedKeyAccess: false
    minimumTlsVersion: 'TLS1_2'
    supportsHttpsTrafficOnly: true
  }
}

// History can be reconstructed by backfill, and pruning is intentional. These
// initial settings do not overwrite a maintainer's existing retention policies.
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
