# Azure infrastructure for production benchmark history

This stack owns the long-lived benchmark history used by collection, backfill and
comparison workflows. It is independent of the throwaway
[test infrastructure](../azure-bench-history-test/), sharing only tenant and
subscription. Production provisioning is a **manual maintainer operation**; none
of the commands below are automatic rollout steps.

## Access model

- The **writer identity**, `id-folo-bench-history-prod` by default, retains
  `Storage Blob Data Contributor` on the storage account. Its `main` branch
  credential supports collection and backfill. New writers default to main-only
  trust; PR analysis uses the reader. Existing writer PR trust is preserved until
  a maintainer explicitly retires it.
- The **reader identity**, by default the writer identity name plus `-reader`,
  receives `Storage Blob Data Reader` on **only the configured history container**,
  not the account, resource group or subscription. It can read/list history but
  cannot write/delete blobs or create containers. A missing container is created
  through the management plane before its role assignment.
- An optional local user or group receives account-scoped
  `Storage Blob Data Contributor`, independently of either CI identity.

Both CI identities use the issuer `https://token.actions.githubusercontent.com`
and audience `api://AzureADTokenExchange`. The reader has these GitHub subjects:

```text
repo:folo-rs/folo:ref:refs/heads/main
repo:folo-rs/folo:pull_request
```

`-GithubOrg` and `-GithubRepo` select the repository. Direct Bicep callers can
configure `githubBranches`, whose default is `main`; writer retirement never
deletes branch credentials. The `pull_request` subject does **not** distinguish
same-repository and fork heads. Workflow policy must restrict reader use to
same-repository PRs and must not expose privileged credentials to fork code.
The absence of stored secrets is not itself an OIDC authorization boundary.

## Prerequisites

- PowerShell 7.6, Azure CLI, and an **already installed Bicep CLI** accessible to
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
- Serialize deployments and retirement for this stack. Do not run them
  concurrently: the state-preserving decisions use a pre-deployment snapshot.

## Deploy the reader first

Run from the repository root, without the retirement switch:

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

az login
.\infra\azure-bench-history-prod\deploy.ps1 `
    -SubscriptionId <subscription-guid> `
    -ResourceGroup folohistory `
    -StorageAccountName folohistory `
    -HistoryContainerName bench-history
```

This creates reader access without retiring writer PR access or changing existing
storage settings/data. Allow Azure RBAC propagation, then verify reader
authentication and history reads before activating replacement PR workflows.
Keep collection/backfill on the writer.

Other parameters (see `deploy.ps1 -?`):

| Flag | Default / purpose |
| --- | --- |
| `-Location` | `swedencentral`; use the existing resources' region on updates |
| `-ManagedIdentityName` | `id-folo-bench-history-prod`; the existing writer |
| `-ReaderManagedIdentityName` | Writer name plus `-reader`; must differ from the writer |
| `-HistoryContainerName` | `bench-history`; must match repository storage configuration |
| `-GithubOrg` / `-GithubRepo` | `folo-rs` / `folo` |
| `-LocalPrincipalId` / `-LocalPrincipalType` | Optional object ID and `User` or `Group` |
| `-RetireWriterPullRequestTrust` | Opt-in staged retirement, described below |

For local data-plane access, optionally include
`-LocalPrincipalId (az ad signed-in-user show --query id -o tsv)`.

### Non-secret output handoff

The script prints these mappings. Record the identifiers in repository
configuration; **do not create secrets** for client, tenant or subscription IDs.
Reader provisioning alone does not activate any workflow.

| Configuration | Bicep output |
| --- | --- |
| `.cargo/bench_history.toml` → `[storage.azure].account` | `storageAccountName` |
| `.cargo/bench_history.toml` → `[storage.azure].container` | `historyContainerName` |
| `AZURE_PROD_CLIENT_ID` in `constants.env` | `managedIdentityClientId` (writer, unchanged) |
| **`AZURE_PROD_READER_CLIENT_ID` in `constants.env`** | **`readerManagedIdentityClientId`** |
| `AZURE_TENANT_ID` / `AZURE_SUBSCRIPTION_ID` | `tenantId` / `subscriptionId` |

`managedIdentityPrincipalId`, `readerManagedIdentityPrincipalId` and `blobEndpoint`
are also available as deployment outputs. Configure the reader client ID before
enabling reader-based workflows.

## Retire the writer's PR credential

**Only after** replacement PR workflows use the reader and all legacy PR-writing
runs have drained, run the same deployment command with the explicit switch:

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

.\infra\azure-bench-history-prod\deploy.ps1 `
    -SubscriptionId <subscription-guid> `
    -ResourceGroup folohistory `
    -StorageAccountName folohistory `
    -HistoryContainerName bench-history `
    -RetireWriterPullRequestTrust
```

Retain any custom identity names and other parameters from reader provisioning.
The switch is the maintainer's confirmation of rollout readiness; the script does
not inspect GitHub runs or revoke already-issued access tokens.

The operation first deploys successfully with writer PR credential creation
disabled, then re-reads the writer's federated credentials. If present, it deletes
**only** `github-pull-request` on the selected writer identity, using the explicit
subscription, resource group and identity name. It never deletes the writer,
reader, branch credentials, role assignments, storage account or history.
Repeating the command is safe: an absent credential needs no delete. Failed
deployment, listing, authorization or deletion is a visible failure, not success.

**Incremental ARM deployments do not delete omitted resources.**
`trustPullRequests=false` only prevents creation; setting it and redeploying alone
does **not** remove an existing writer PR credential. The explicit CLI deletion
performed by `-RetireWriterPullRequestTrust` is what retires trust.

## Deployment behavior

`deploy.ps1` calls the Pester-tested `ProductionIdentityDeployment.psm1` module.
This is a thin Azure CLI provisioning boundary usable without a Rust toolchain;
Bicep remains the resource definition authority.

- **Existing storage:** successful management-plane listings select
  `createStorageAccount=false` and, if present, `createHistoryContainer=false`.
  The account, blob service and existing container are Bicep `existing`
  references: their properties, retention settings and data are not overwritten.
- **Fresh storage:** bootstrap modules create an Entra-only `StorageV2`
  `Standard_LRS` account (HTTPS, TLS 1.2, no public blobs or shared-key access),
  initially disable container/blob soft delete, and create the private history
  container before assigning its reader role.
- **Existing writer:** deployment preserves whether `github-pull-request`
  currently exists. After retirement, its absence is sufficient state; subsequent
  ordinary wrapper deployments do not recreate it. No tag or recurring
  retirement flag is required.
- **Fresh writer:** branch trust defaults to `main`, without PR trust, whether
  storage is new or already exists. PR analysis uses the reader. Recreating a
  writer therefore needs no retirement switch to keep writer PR trust disabled.
- **Incremental mode:** unmentioned resources and optional local grants remain.
  Omission is not a general-purpose resource deletion mechanism.

Always use the wrapper for routine deployments. Direct Bicep/ARM callers bypass
its state discovery. They must explicitly select the bootstrap flags.
`main.bicep` defaults `trustPullRequests=false`: this prevents writer PR credential
creation but does not delete existing trust in incremental mode. The optional
`main.bicepparam` requires explicit
`AZURE_CREATE_STORAGE_ACCOUNT`, `AZURE_CREATE_HISTORY_CONTAINER` and
`AZURE_TRUST_PULL_REQUESTS` Boolean environment values. It also accepts
`AZURE_HISTORY_CONTAINER_NAME` and `AZURE_READER_MANAGED_IDENTITY_NAME`.
Bootstrap flags must be true only for missing resources.

## Local collection and destructive teardown

Local collection uses your own Entra principal:

```powershell
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

az login
cargo run -p cargo-bench-history --bin cargo-bench-history -- collect --workspace --exclude benchmarks
```

This writes the configured production account and requires local contributor
access. Use `--local=<path>` for a throwaway run that never accesses Azure.

`teardown.ps1` deletes the entire production resource group and **permanently
deletes collected history**, both identities and their access configuration.
It is not part of reader provisioning or writer-PR retirement. Reconstructing
history requires backfill; ordinary collection only adds history going forward.
