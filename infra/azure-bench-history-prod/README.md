# Azure infrastructure for production benchmark history

This stack owns the long-lived benchmark history used by collection, backfill and
comparison workflows. It is independent of the throwaway
[test infrastructure](../azure-bench-history-test/), sharing only tenant and
subscription. Production provisioning is a **manual maintainer operation**; none
of the commands below are automatic rollout steps.

## Access model

- One **production identity**, `id-folo-bench-history-prod` by default, has
  `Storage Blob Data Contributor` on the storage account. Collection, backfill and
  analysis share it; there is no separate reader role or client ID.
- An optional local user or group receives account-scoped
  `Storage Blob Data Contributor`, independently of the production identity.

The production identity uses the issuer `https://token.actions.githubusercontent.com`
and audience `api://AzureADTokenExchange`, with these GitHub subjects:

```text
repo:folo-rs/folo:ref:refs/heads/main
repo:folo-rs/folo:pull_request
```

`-GithubOrg`, `-GithubRepo` and `-HistoryBranch` select the repository and branch.
The Folo wrapper defaults to `main`. The `pull_request` subject does **not**
distinguish same-repository and fork heads. Workflow policy must restrict identity use to
same-repository PRs and must not expose privileged credentials to fork code.
The absence of stored secrets is not itself an OIDC authorization boundary.

Analysis and GitHub publication may run in one job. Azure access uses this identity;
issue/comment access uses the job's built-in GitHub token. PR and trunk measurements use
the configured store; collection artifacts carry receipts rather than a second copy of data.

## Prerequisites

- The repository's Rust development environment, PowerShell 7.6, Azure CLI, and an
  **already installed Bicep CLI** accessible to
  Azure CLI. `az bicep version` must succeed. The deployment wrapper checks this
  before any Azure changes and does not install tooling.
- `az login` as a maintainer with resource provisioning and role-assignment rights
  in the selected subscription/resource group, including managed identity
  federated-credential management. Owner, or Contributor combined with User Access
  Administrator at the appropriate scope, are examples. User Access Administrator
  alone does not grant resource provisioning rights.
- Confirm the target subscription, resource group, account and container.
  [`.cargo/bench_history.toml`](../../.cargo/bench_history.toml) configures
  account `folohistory`, container **`bench-history`**. The wrapper defaults match.
- Serialize deployments targeting the same storage account or managed identity
  in the selected subscription and resource group: the state-preserving decisions
  use a pre-deployment snapshot.

## Deploy the production stack

Run from the repository root, replacing the quoted subscription placeholder:

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

az login
.\infra\azure-bench-history-prod\deploy.ps1 `
    -SubscriptionId '<subscription-guid>' `
    -ResourceGroup folohistory `
    -StorageAccountName folohistory `
    -HistoryContainerName bench-history
```

This provisions storage and one identity without changing existing storage settings or data.
Allow Azure RBAC propagation, then verify authentication and storage access before activating
the workflows. A correctly configured existing production identity can be reused.

Other parameters (see `deploy.ps1 -?`):

| Flag | Default / purpose |
| --- | --- |
| `-Location` | `swedencentral`; use the existing resources' region on updates |
| `-ManagedIdentityName` | `id-folo-bench-history-prod`; shared production identity |
| `-HistoryContainerName` | `bench-history`; must match repository storage configuration |
| `-GithubOrg` / `-GithubRepo` | `folo-rs` / `folo` |
| `-HistoryBranch` | `main`; branch allowed to federate alongside PRs |
| `-LocalPrincipalId` / `-LocalPrincipalType` | Optional object ID and `User` or `Group` |

For local data-plane access, optionally include
`-LocalPrincipalId (az ad signed-in-user show --query id -o tsv) -LocalPrincipalType User`.

### Non-secret output handoff

The script prints these mappings. Record the identifiers in repository
configuration; **do not create secrets** for client, tenant or subscription IDs.
Provisioning alone does not activate any workflow.

| Configuration | Bicep output |
| --- | --- |
| `.cargo/bench_history.toml` → `[storage.azure].account` | `storageAccountName` |
| `.cargo/bench_history.toml` → `[storage.azure].container` | `historyContainerName` |
| `AZURE_PROD_CLIENT_ID` in `constants.env` | `managedIdentityClientId` |
| `AZURE_TENANT_ID` / `AZURE_SUBSCRIPTION_ID` | `tenantId` / `subscriptionId` |

`managedIdentityPrincipalId` and `blobEndpoint` are also available as deployment outputs.

## Deployment behavior

`deploy.ps1` supplies Folo defaults to `cargo run -p cargo-bench-history --bin cargo-bench-history
--locked -- setup-azure`. Routine production provisioning therefore exercises the source-built
CLI, including its prerequisite checks and embedded
[canonical deployment bundle](../../packages/cargo-bench-history/src/azure_bundle/).
The exported bundle remains usable without a Rust toolchain; its driver calls the
Pester-tested `ProductionIdentityDeployment.psm1` module. Bicep remains the resource
definition authority.

- **Existing storage:** successful management-plane listings select
  `createStorageAccount=false` and, if present, `createHistoryContainer=false`.
  The account, blob service and existing container are Bicep `existing`
  references: their properties, retention settings and data are not overwritten.
- **Fresh storage:** bootstrap modules create an Entra-only `StorageV2`
  `Standard_LRS` account (HTTPS, TLS 1.2, no public blobs or shared-key access),
  initially disable container/blob soft delete, and create the private history container.
- **Identity:** fresh and repeated deployments ensure the same production identity,
  `Storage Blob Data Contributor` role and configured branch/PR federated subjects.
- **Incremental mode:** unmentioned resources and optional local grants remain.
  Omission is not a general-purpose resource deletion mechanism.

Always use the wrapper for routine deployments. Direct Bicep/ARM callers bypass
its state discovery. They must explicitly select `createStorageAccount` and
`createHistoryContainer`, setting each to true only when its corresponding resource is absent.
The bundle's `parameters.json` is standalone driver input, with explicit required
placement and repository values rather than Folo defaults.

## Local collection and destructive teardown

Local collection uses your own Entra principal:

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

az login
cargo run -p cargo-bench-history --bin cargo-bench-history -- collect --workspace --exclude benchmarks
```

This writes to the configured production storage account and requires local
`Storage Blob Data Contributor` access. Use `--local=<path>` for a throwaway run
that never accesses Azure.

`teardown.ps1` deletes the entire production resource group and **permanently
deletes collected history**, the identity and its access configuration.
It is not part of ordinary provisioning. Reconstructing
history requires backfill; ordinary collection only adds history going forward.
