# Azure infrastructure for cargo-bench-history real-Azure tests

This directory provisions the Azure resources that back the `cargo-bench-history`
real-Azure storage tests (the `test-azure` CI job and local runs). Everything is
described in Bicep and driven by idempotent PowerShell scripts, so the environment
can be deleted and re-created with one command.

## What gets created

`main.bicep` (deployed at resource-group scope) creates:

- A **Storage account** — `StorageV2`, HTTPS-only, TLS 1.2, **shared-key access
  disabled** (Entra ID only, so there is no account key to leak). Container and blob
  soft-delete are disabled so a deleted test container is gone immediately.
- A **user-assigned managed identity** — the CI principal — with **GitHub OIDC
  federated credentials** (one per trusted branch, plus same-repo pull requests).
  GitHub Actions signs in with no stored secret.
- **`Storage Blob Data Contributor`** role assignments on the account for the managed
  identity and (optionally) a local developer principal. That single role covers
  container create/delete and blob read/write/delete via the data plane, which is all
  the tool's `run` and the tests' container cleanup need.

## Prerequisites

- Azure CLI (`az`) and PowerShell 7+.
- `az login` as an account allowed to create these resources and assign roles
  (Owner or User Access Administrator on the target scope).

## Deploy

```powershell
# Your own object id for local data-plane access (optional but recommended):
$me = az ad signed-in-user show --query id -o tsv

./deploy.ps1 `
    -SubscriptionId <subscription-guid> `
    -StorageAccountName <globally-unique-name> `
    -LocalPrincipalId $me
```

Key parameters (see `deploy.ps1 -?` for all): `-ResourceGroup` (default
`rg-folo-bench-history`), `-Location` (default `swedencentral`), `-StorageAccountName`
(3-24 lowercase alphanumerics, globally unique), `-LocalPrincipalId` /
`-LocalPrincipalType` (`User` or `Group`).

On success the script prints the identifiers to record in `constants.env`.

## Configure the repository

The Azure identifiers are committed (non-secret) in `constants.env` at the repository
root, shared by local `just test-azure` runs and the CI `test-azure` job so both
target the same account. If you re-created the resources, update these lines to match
the values the deploy script printed:

| Key | Source |
| --- | --- |
| `BENCH_HISTORY_TEST_AZURE_ACCOUNT` | storage account name |
| `AZURE_CLIENT_ID` | managed identity client id |
| `AZURE_TENANT_ID` | tenant id |
| `AZURE_SUBSCRIPTION_ID` | subscription id |

These are identifiers, not credentials — authentication is via Microsoft Entra ID
(local `az login` / CI OIDC federation), so there is nothing to leak by committing
them. They do not by themselves run the real-Azure tests: those run only when
`ENABLE_AZURE` is set, which the `just test-azure` recipe does. Pull requests from
forks cannot mint a token for the tenant and so the CI job skips for them.

## Run the tests locally

```powershell
az login                       # sign in as your Entra user
just test-azure                # uses BENCH_HISTORY_TEST_AZURE_ACCOUNT from constants.env
```

`just test-azure` sets `ENABLE_AZURE=1` (which opts the real-Azure tests in and makes
a misconfigured account fail loudly rather than skip) and runs the `real_azure`
tests. Pass a name to target a different account: `just test-azure <storage-account-name>`.

Without `ENABLE_AZURE` (for example under a plain `just test`) the real-Azure tests
self-skip regardless of the account, so they never target the cloud unintentionally;
the Azurite tests are unaffected. With `ENABLE_AZURE` set (as `just test-azure` does)
a missing `BENCH_HISTORY_TEST_AZURE_ACCOUNT` is instead a hard failure, not a skip. Each
test creates a unique `bh-it-*` container and deletes it when it finishes.

## Clean up leftover containers

A crashed or timed-out test can leave a container behind. Sweep them with:

```powershell
./cleanup-containers.ps1 -AccountName <storage-account-name>
```

The CI job runs this automatically (`if: always()`).

## Tear down / re-create

```powershell
./teardown.ps1 -SubscriptionId <subscription-guid>     # deletes the resource group
./deploy.ps1 ...                                        # re-create from scratch
```
