# setup-azure

Shared history needs durable storage and a CI identity. `setup-azure` provisions an
Azure account, private container and one GitHub-federated managed identity for collection,
backfill and analysis. It works without a checkout or benchmark configuration.

## Review the deployment first

```text
cargo bench-history setup-azure --out-dir ./azure-history
```

Export only writes the bundle: it does not start processes, check prerequisites, contact
Azure or require login. The destination must be absent or empty; relative paths start
at the invocation directory. Edit `parameters.json`, supplying its required values, then
follow the exported README to run `deploy.ps1` directly. No Rust toolchain or Folo
checkout is needed to use the exported files, and no teardown is included.

## Provision directly

After separately installing Azure CLI, PowerShell 7.6 or later and Bicep, authenticate
Azure CLI with permission to provision resources, assign roles and configure federation.
The command checks those prerequisites without installing tools or initiating login.
It requires an explicit target rather than choosing the active subscription:

```text
cargo bench-history setup-azure --subscription-id <subscription-guid> --resource-group team-history --location westeurope --storage-account <unique-account-name> --github-owner example --github-repository project --history-branch main
```

The container defaults to `bench-history`, and the identity name derives from the account.
Use `--managed-identity` to reuse a differently named identity. Optional local access uses
`--local-principal-id <object-id> --local-principal-type user` (or `group`) together.
Use `--help` for the complete grouped parameter reference.

Fresh and repeated deployments ensure account-scoped Storage Blob Data Contributor and
the configured branch and PR federation subjects. Existing storage settings and history
are preserved. Serialize deployments of the same stack. Failures retain diagnostics;
completed Azure changes are not rolled back.

The PR subject cannot distinguish fork heads. Your workflows must restrict privileged
identity use to same-repository pull requests.

## Connect the result

Successful deployment prints `[storage.azure]` settings for `.cargo/bench_history.toml`,
the blob endpoint and non-secret tenant, subscription, client and principal IDs.
Use the client, tenant and subscription IDs in workflow OIDC login configuration;
do not store them as credentials. The command does not edit the caller's configuration
or GitHub settings. See [storage backends](../storage.md) for runtime storage selection.
