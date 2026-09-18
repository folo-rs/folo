# Azure benchmark history deployment

This bundle is independently executable; no Folo checkout or Rust toolchain is needed.
It contains no teardown command.

## Deploy

Keep `parameters.json` as a JSON object using the provided parameter names.
Edit it and supply every `null` required value: subscription ID,
resource group, location, storage account, GitHub owner/repository and history branch.
Optional identity name defaults to `id-<storage-account>-bench-history`. Optional local
access requires both an object ID and type (`User` or `Group`). The container defaults
to `bench-history`. Values in this JSON file are literal data, not shell expressions.
Supply a literal [Git branch name](https://git-scm.com/docs/git-check-ref-format)
such as `main` or `release/next`, not a qualified reference such as `refs/heads/main`.

Install Azure CLI, PowerShell 7.6 or later and Bicep separately, then authenticate
Azure CLI for the explicitly chosen subscription. The driver does not install tools or
initiate login. The authenticated principal needs resource provisioning, role assignment and federated
credential management privileges.

```powershell
pwsh -NoProfile -NonInteractive -File ./deploy.ps1 -ParametersFile ./parameters.json
```

Explicit script flags override parameter-file values. The driver fails on missing
inputs before probing tools, and checks tooling and authentication before mutations.
It prints non-secret storage settings and identity IDs for configuration; it does not
edit repositories, GitHub settings or credentials.

## Deployment behavior

One managed identity has account-scoped Storage Blob Data Contributor and GitHub
history-branch plus pull-request federation. Optional local access grants the same role
independently. GitHub PR subjects cannot distinguish fork heads: workflows must gate
identity use to same-repository PRs.

Successful management-plane listings select creation only for missing storage.
Existing account, blob-service and container settings and data remain untouched.
New storage is private and Entra-only. Deployment is incremental; unmentioned
resources and local grants remain. Serialize invocations targeting the same storage account
or managed identity in the selected subscription and resource group.

Failures remain failures, with child diagnostics. Already completed Azure changes
are not rolled back. Direct Bicep users must set `createStorageAccount` and
`createHistoryContainer` to true only when the corresponding resource is absent; routine
deployments use the driver to perform this discovery safely.
