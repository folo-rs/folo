# Azure infrastructure for cargo-bench-history real-Azure tests

This directory provisions the Azure resources that back the `cargo-bench-history`
real-Azure storage tests (the `test-azure` CI job and local runs).
`deploy.ps1` is a thin wrapper around the source-built
`cargo-bench-history setup-azure` command, supplying test-specific parameters.
Production and test deployment use the same
[package-owned templates](../../packages/cargo-bench-history/src/azure_bundle/).

## What gets created

The shared deployment creates or reuses:

- A **Storage account**. New accounts use `StorageV2`, HTTPS-only, TLS 1.2 and
  **disabled shared-key access** (Entra ID only, so there is no account key to leak).
  Container and blob soft-delete are initially disabled for immediate cleanup.
- A **user-assigned managed identity** — the CI principal — with **GitHub OIDC
  federated credentials** for the selected branch and repository pull-request
  subject. GitHub's default fork-PR permissions prevent OIDC issuance;
  the PR subject itself does not distinguish fork heads.
  GitHub Actions signs in with no stored secret.
- A **named container**, `bench-history` by default. The test scenarios use their own
  fresh `bh-it-*` containers instead, which the cleanup script removes independently.
- **`Storage Blob Data Contributor`** role assignments on the account for the managed
  identity and (optionally) an additional user or group. That single role covers
  container create/delete and blob read/write/delete via the data plane, which is all
  the tool's collection and the tests' container cleanup need.

## Prerequisites

- The repository's Rust development environment, plus
  [Azure CLI](https://learn.microsoft.com/cli/azure/install-azure-cli),
  [PowerShell 7.6 or later](https://learn.microsoft.com/powershell/scripting/install/installing-powershell)
  and [installed Bicep](https://learn.microsoft.com/azure/azure-resource-manager/bicep/install#azure-cli).
- `az login` as an account allowed to create these resources and assign roles
  (Owner, or Contributor combined with User Access Administrator on the target
  scope). User Access Administrator alone cannot provision resources.

The command checks software, authentication and optional current-user lookup before
resource-group creation. The wrapper performs no separate Azure operations.
Provisioning never installs tools, initiates login or changes the active CLI subscription.

## Deploy

```powershell
az login
az account set --subscription '<subscription-guid>'
./deploy.ps1 `
    -SubscriptionId '<subscription-guid>' `
    -StorageAccountName '<globally-unique-name>' `
    -CurrentUser
```

Key parameters (see `deploy.ps1 -?` for all): `-ResourceGroup` (default
`rg-folo-bench-history`), `-Location` (default `swedencentral`), `-StorageAccountName`
(3-24 lowercase alphanumerics, globally unique), `-CustomPrincipalId` /
`-CustomPrincipalType` (`User` or `Group`). `-ManagedIdentityName` defaults to
`id-folo-bench-history-ci`, separately from the production identity.
`-HistoryBranch` defaults to `main`; `-HistoryContainerName` defaults to `bench-history`.

`-CurrentUser` resolves your signed-in user in the explicitly selected
subscription's tenant, including a guest user's object in that tenant. It requires
a user login, Microsoft Graph access and the target subscription active in Azure CLI,
and conflicts with either custom-principal
flag. Alternatively, supply both custom flags with an existing user/group object
ID from the target tenant. Neither path creates an Entra principal.

On success the command prints non-secret deployment outputs and the wrapper explains
their test-specific mapping to `constants.env`. Do not replace the production storage
configuration in `.cargo/bench_history.toml` with the test account.

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
forks cannot obtain effective `id-token: write` under GitHub's default permissions,
even when they edit YAML to request it. Maintainer approval does not elevate that
permission. The job also skips forks explicitly through its same-repository gate.
The Azure `repo:<owner>/<repository>:pull_request` subject is not itself a fork
boundary. Fork-owned workflows identify the fork repository and do not match it.

## Relationship to production storage

Both deployment wrappers use the same `setup-azure` command. Account, group and
managed-identity parameters provide isolation; separate templates or test modes are
not needed. Each identity receives contributor access only to its selected account.
Their credential children can both use `bench-history-branch` and
`bench-history-pull-request` because the parent identities are separate.

Fresh and repeated deployments share the production preservation policy: existing
account/container settings and data remain untouched, and additional custom-principal
grants do not remove earlier grants. Repeating setup does not reset manually changed
test retention settings. Test scenarios create and delete their own containers;
the explicit teardown below is destructive.

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
