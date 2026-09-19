# Azure benchmark history deployment

This bundle provisions a durable history store and one GitHub-federated identity.
It is independently executable: no Folo checkout or Rust toolchain is needed.
It contains no teardown command.

## Deploy

Install the prerequisites using their supported installation instructions:

- [Azure CLI](https://learn.microsoft.com/cli/azure/install-azure-cli).
- [PowerShell](https://learn.microsoft.com/powershell/scripting/install/installing-powershell),
  version 7.6 or later.
- [Bicep for Azure CLI](https://learn.microsoft.com/azure/azure-resource-manager/bicep/install#azure-cli).
  `az bicep version` must succeed before deployment.

[Sign in to Azure CLI](https://learn.microsoft.com/cli/azure/authenticate-azure-cli-interactively)
for your target subscription's tenant. The authenticated principal needs resource
provisioning, role-assignment and federated-credential-management privileges.
Owner, or Contributor combined with User Access Administrator at the relevant
scope, are examples; User Access Administrator alone cannot provision resources.
The driver checks prerequisites but does not install tools or initiate login.

Keep `parameters.json` as a JSON object with the provided parameter names. Supply
every required `null` value: subscription ID, resource group, location, storage
account, GitHub owner/repository and history branch. The subscription and GitHub
repository must already exist. The group, storage account, container and managed
identity are created or reused. `deploy.ps1 -?` documents each parameter's meaning,
requiredness and example format.

The optional identity name defaults to `id-<storage-account>-bench-history`.
The container defaults to `bench-history`. To grant an additional existing Entra
user or group access, supply both `CustomPrincipalId` (its object ID in the
subscription's tenant) and `CustomPrincipalType` (`User` or `Group`). These are
not application/client IDs, and the deployment does not create that principal.
Alternatively, leave those values null and add `-CurrentUser` when running the
driver. It resolves your Azure CLI signed-in user in the selected subscription's
tenant before any Azure changes; a service-principal login cannot use it.
Lookup requires Microsoft Graph access and the selected subscription to be active
in Azure CLI. If necessary, run `az account set --subscription <subscription-guid>`
before using `-CurrentUser`; the driver checks this match without changing your
CLI default.

Values in the JSON file are literal data, not shell expressions. Supply a literal
[Git branch name](https://git-scm.com/docs/git-check-ref-format) such as `main` or
`release/next`, not `refs/heads/main`.

```powershell
pwsh -NoProfile -NonInteractive -File ./deploy.ps1 -ParametersFile ./parameters.json
```

Explicit script flags override parameter-file values. To also grant yourself access:

```powershell
pwsh -NoProfile -NonInteractive -File ./deploy.ps1 -ParametersFile ./parameters.json -CurrentUser
```

`-CurrentUser` conflicts with either custom principal value, including values from
the parameter file. Missing inputs fail before tool probes. Tooling, authentication
and requested identity lookup are checked before resource mutations.

## Configure the repository and workflows

The driver prints non-secret settings and identifiers. It does not edit repositories,
GitHub settings or credentials.

| Output | Destination or purpose |
| --- | --- |
| `[storage.azure]` account and container | Copy the section into `.cargo/bench_history.toml` and commit it. |
| Managed identity client ID | GitHub Actions repository variable `AZURE_CLIENT_ID`; use it to set job environment variable `AZURE_CLIENT_ID`. |
| Azure tenant ID | Repository variable `AZURE_TENANT_ID`; use it to set job environment variable `AZURE_TENANT_ID`. |
| Azure subscription ID | Future deployments/administration, or `subscription-id` in an optional `azure/login` step. Direct benchmark OIDC does not need it. |
| Managed identity principal ID | Inspect Azure role assignments for access diagnostics. Do not use this as the client ID. |
| Blob endpoint | Connectivity diagnostics or Azure tools; no extra benchmark configuration field. |

Create the repository variables under **Settings → Secrets and variables → Actions
→ Variables**, not Secrets. In the job running the root composite action or direct
benchmark commands, configure:

```yaml
env:
  AZURE_CLIENT_ID: ${{ vars.AZURE_CLIENT_ID }}
  AZURE_TENANT_ID: ${{ vars.AZURE_TENANT_ID }}
```

Grant that job `id-token: write` alongside its other benchmark-flow permissions.
The root action inherits the job environment. The tool exchanges OIDC tokens
itself without a separate login action.
The [automation guide](https://folo-rs.github.io/folo/cargo-bench-history/github-automation.html)
provides complete caller examples.

## Security model

One managed identity has account-scoped Storage Blob Data Contributor. It can
read/write/delete blobs and create/delete containers across the account, not only
the configured history container. An optional custom principal or current user
receives the same role independently.

`GithubOrg`, `GithubRepo` and `HistoryBranch` configure:

```text
repo:<GithubOrg>/<GithubRepo>:ref:refs/heads/<HistoryBranch>
repo:<GithubOrg>/<GithubRepo>:pull_request
```

The issuer is `https://token.actions.githubusercontent.com`; the audience is
`api://AzureADTokenExchange`. Branch trust is limited to the selected branch;
PR trust uses the repository's PR event context and is not limited to that target
branch. These subjects do not limit access to a particular workflow file or action.
Environment jobs or customized GitHub subject formats need matching trust settings.
See [GitHub's OIDC subject reference](https://docs.github.com/en/actions/reference/security/oidc#example-subject-claims).

The PR workflow in the automation guide skips forks before credentialed work. Keep its
same-repository gate: the PR subject itself does not distinguish fork heads.
Custom PR jobs must check
`github.event.pull_request.head.repo.full_name == github.repository` before using
the identity. Only grant `id-token: write` where needed. Code and actions running
with the identity are trusted with its storage rights; GitHub issue/comment rights
come separately from the job's `GITHUB_TOKEN`.
Fork benchmarking remains unsupported without secure federated access to the base
repository's history; stored credentials are not a substitute.

## Deployment behavior

Successful management-plane listings select creation only for missing storage.
Existing account, blob-service and container settings and data remain untouched.
New storage is private and Entra-only.

Deployment is incremental. Changing the custom principal adds a grant without
removing earlier grants; omission does not revoke access. Unmentioned resources
remain. Federation is configurable, not append-only: changing repository or
history-branch inputs updates the selected identity's existing credentials.
The stable `github-branch-main` resource key uses the selected `HistoryBranch`
even when it is not `main`; changing the branch replaces that credential's subject.
Serialize invocations targeting the same storage account or managed identity
in the selected subscription and resource group.

Failures remain failures, with child diagnostics. Already completed Azure changes
are not rolled back; resolve the failure and rerun with the intended inputs.
Direct Bicep users must set `createStorageAccount` and `createHistoryContainer`
to true only when the corresponding resource is absent. Routine deployments use
the driver to perform this discovery safely.
