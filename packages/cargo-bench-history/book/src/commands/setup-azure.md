# setup-azure

Shared history needs durable storage and a managed identity for its workflows.
`setup-azure` provisions an Azure Storage account, a history container and one
GitHub-federated managed identity for collection, backfill and analysis. Newly created
storage is private and Entra-only. The command works without a checkout or benchmark
configuration.

A managed identity is an Azure principal to which roles grant access. GitHub Actions can
authenticate as it without stored secrets using OpenID Connect (OIDC) federation: Azure
trusts configured subject claims in GitHub's short-lived tokens. A subject identifies the
repository and workflow event context.

## Review the deployment first

Deployment parameters are optional with `--out-dir`. Supplied values prepopulate
`parameters.json`; omitted required values remain for you to fill in before deployment:

```text
cargo bench-history setup-azure --out-dir ./azure-history --storage-account examplehistory --history-branch main
```

Export only writes the bundle: it does not start processes, check prerequisites, contact
Azure or require login. The destination must be absent or empty; relative paths start
at the invocation directory. Review `parameters.json` and fill its remaining required values, then
follow the exported README to run `deploy.ps1` directly. No Rust toolchain or Folo
checkout is needed to use the exported files, and no teardown is included.

## Provision directly

After separately installing Azure CLI, PowerShell 7.6 or later and Bicep, authenticate
Azure CLI with permission to provision resources, assign roles and configure federation.
The command checks those prerequisites without installing tools or initiating login.
It requires an explicit target rather than choosing the active subscription.
Replace the quoted placeholders with your subscription ID and a globally unique storage
account name:

```powershell
cargo bench-history setup-azure --subscription-id '<subscription-guid>' --resource-group team-history --location westeurope --storage-account '<unique-account-name>' --github-owner example --github-repository project --history-branch main
```

The container defaults to `bench-history`, and the managed identity name derives from the
selected storage account. Use `--managed-identity` to reuse a differently named managed
identity. Optional local access uses
`--local-principal-id <object-id> --local-principal-type user` (or `group`) together.
Use `--help` for the complete grouped parameter reference.

Fresh and repeated deployments ensure the managed identity has an account-scoped
Storage Blob Data Contributor role assignment and federated identity credentials for the
configured history-branch and pull-request subjects. Existing storage settings and history
are preserved. Serialize invocations targeting the same storage account or managed identity
in the selected subscription and resource group. Failures retain diagnostics; completed Azure
changes are not rolled back.

The PR subject cannot distinguish fork heads. Your workflows must restrict privileged
identity use to same-repository pull requests.

## Connect the result

Successful deployment prints `[storage.azure]` settings for `.cargo/bench_history.toml`,
the blob endpoint and non-secret tenant, subscription, client and principal IDs.
Use the client, tenant and subscription IDs in workflow OIDC login configuration;
do not store them as credentials. The command does not edit the caller's configuration
or GitHub settings. See [storage backends](../storage.md) for runtime storage selection.
