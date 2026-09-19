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
  federated credentials** (one per trusted branch, plus an optional repository
  pull-request subject). Workflow policy gates out forks before identity use;
  the PR subject itself does not distinguish fork heads.
  GitHub Actions signs in with no stored secret.
- **`Storage Blob Data Contributor`** role assignments on the account for the managed
  identity and (optionally) an additional user or group. That single role covers
  container create/delete and blob read/write/delete via the data plane, which is all
  the tool's `run` and the tests' container cleanup need.

## Prerequisites

- [Azure CLI](https://learn.microsoft.com/cli/azure/install-azure-cli),
  [PowerShell 7.6 or later](https://learn.microsoft.com/powershell/scripting/install/installing-powershell)
  and [installed Bicep](https://learn.microsoft.com/azure/azure-resource-manager/bicep/install#azure-cli).
- `az login` as an account allowed to create these resources and assign roles
  (Owner, or Contributor combined with User Access Administrator on the target
  scope). User Access Administrator alone cannot provision resources.

The script shares read-only prerequisite and current-user checks with the
production bundle. These run before resource-group creation and never install
tools, initiate login or change the active CLI subscription.

## Deploy

```powershell
./deploy.ps1 `
    -SubscriptionId <subscription-guid> `
    -StorageAccountName <globally-unique-name> `
    -CurrentUser
```

Key parameters (see `deploy.ps1 -?` for all): `-ResourceGroup` (default
`rg-folo-bench-history`), `-Location` (default `swedencentral`), `-StorageAccountName`
(3-24 lowercase alphanumerics, globally unique), `-CustomPrincipalId` /
`-CustomPrincipalType` (`User` or `Group`).

`-CurrentUser` resolves your signed-in user in the explicitly selected
subscription's tenant, including a guest user's object in that tenant. It requires
a user login, Microsoft Graph access and the target subscription active in Azure CLI,
and conflicts with either custom-principal
flag. Alternatively, supply both custom flags with an existing user/group object
ID from the target tenant. Neither path creates an Entra principal.

On success the script prints the identifiers to record in `constants.env`.

## Configure the repository

The Azure identifiers are committed (non-secret) in `constants.env` at the repository
root, shared by local `just test-azure` runs and the CI `test-azure` job so both
target the same account. If you re-created the resources, update these lines to match
the values the deploy script printed:

| Key | Source |
| --- | --- |
| `BENCH_HISTORY_TEST_AZURE_ACCOUNT` | storage account name |
| `AZURE_TEST_CLIENT_ID` | managed identity client id |
| `AZURE_TENANT_ID` | tenant id |
| `AZURE_SUBSCRIPTION_ID` | subscription id |

These are identifiers, not credentials — authentication is via Microsoft Entra ID
(local `az login` / CI OIDC federation), so there is nothing to leak by committing
them. They do not by themselves run the real-Azure tests: those run only when
`ENABLE_AZURE` is set, which the `just test-azure` recipe does. Pull requests from
forks are skipped by an explicit same-repository workflow gate. The Azure
`repo:<owner>/<repository>:pull_request` subject is not itself a fork boundary.

## Relationship to production storage

Tooling/authentication checks, current-user resolution and custom-principal naming
are shared with production. Both identities use account-scoped Storage Blob Data
Contributor and serialize federated-credential creation on each identity.

Their resource lifecycles are intentionally different. Test Bicep owns disposable
account properties and reapplies disabled soft-delete settings on deployment; tests
create and delete their own containers. Production provisioning preserves existing
account/container properties and durable history. Test deployment retains its
separate identity, configurable trusted-branch list and optional PR trust; it does
not acquire the production identity or access to production history.

Test deployment is incremental: additional custom-principal grants do not remove
earlier grants. This is not a promise to preserve manually changed test account
settings. The explicit teardown below is destructive.

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
