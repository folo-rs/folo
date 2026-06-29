# Azure infrastructure for the nightly benchmark-history data store

This directory provisions the Azure storage account that holds the **real,
long-lived benchmark history** collected by the `bench-history` GitHub workflow
(and by local `just collect-bench-history` runs). It is the production data store,
as opposed to the throwaway test account in
[`infra/azure-bench-history-test/`](../azure-bench-history-test/).

Everything is described in Bicep and driven by idempotent PowerShell scripts, so
the environment can be deleted and re-created with one command.

## What gets created

`main.bicep` (deployed at resource-group scope) creates:

- A **Storage account** — `StorageV2`, HTTPS-only, TLS 1.2, **shared-key access
  disabled** (Entra ID only, so there is no account key to leak). Container and blob
  soft-delete are disabled, matching the test account.
- A **dedicated user-assigned managed identity** (`id-folo-bench-history-prod`) with a
  **GitHub OIDC federated credential** for `main`. The nightly workflow signs in with
  no stored secret. Only `main` is trusted — there is no pull-request credential —
  because collection only ever runs on `main` (schedule + gated dispatch).
- **`Storage Blob Data Contributor`** role assignments on the account for that managed
  identity and (optionally) a local developer principal. That single role covers
  container create/delete and blob read/write/delete via the data plane, which is all
  the tool's `run` (and any later `prune`) need.

This stack is **fully self-contained**: it owns its own identity and shares nothing
with [`infra/azure-bench-history-test/`](../azure-bench-history-test/) except the
tenant and subscription. Either can be deployed, torn down, and re-created
independently — keeping the prod data store from depending on test infrastructure.

## Prerequisites

- Azure CLI (`az`) and PowerShell 7+.
- `az login` as an account allowed to create these resources and assign roles
  (Owner or User Access Administrator on the target scope).

## Deploy

```powershell
# Your own object id for local data-plane access (optional but recommended, so you
# can inspect/manage the history with the tool or `az`):
$me = az ad signed-in-user show --query id -o tsv

./deploy.ps1 `
    -SubscriptionId <subscription-guid> `
    -LocalPrincipalId $me
```

Key parameters (see `deploy.ps1 -?` for all): `-ResourceGroup` (default
`folohistory`), `-Location` (default `swedencentral`), `-StorageAccountName`
(default `folohistory`; 3-24 lowercase alphanumerics, globally unique),
`-ManagedIdentityName` (default `id-folo-bench-history-prod`),
`-LocalPrincipalId` / `-LocalPrincipalType`.

On success the script prints the identifiers to record in `constants.env`.

## Configure the repository

The account name and the prod identity's client id are committed (non-secret) in
`constants.env` at the repository root. The tenant and subscription are shared with
the test identity. If you re-created the resources, update these lines to match the
values the deploy script printed:

| Key | Source |
| --- | --- |
| `BENCH_HISTORY_PROD_AZURE_ACCOUNT` | storage account name (this deployment) |
| `AZURE_PROD_CLIENT_ID` | this deployment's managed identity client id |
| `AZURE_TENANT_ID` / `AZURE_SUBSCRIPTION_ID` | shared tenant/subscription (already present) |

## Collect history locally

```powershell
az login                        # sign in as your Entra user
just collect-bench-history      # benches the workspace and stores into Azure
```

`just collect-bench-history` benches every workspace package except `benchmarks`
(the slow, special-purpose one) and stores the results in the
`BENCH_HISTORY_PROD_AZURE_ACCOUNT` account. It requires your user to hold the
`Storage Blob Data Contributor` role on the account (deploy with `-LocalPrincipalId`
as above). Pass an account name to target a different one:
`just collect-bench-history <storage-account-name>`.

## Tear down / re-create

```powershell
./teardown.ps1 -SubscriptionId <subscription-guid>     # deletes the resource group
./deploy.ps1 ...                                        # re-create from scratch
```

Tearing down permanently deletes the collected history. It is reconstructible with
`cargo bench-history backfill` over past commits, but the nightly job otherwise only
repopulates history going forward.
