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
| Managed identity client ID | GitHub Actions repository variable `AZURE_CLIENT_ID`; pass it as the prebuilt workflows' `azure-client-id` input. |
| Azure tenant ID | Repository variable `AZURE_TENANT_ID`; pass it as `azure-tenant-id`. |
| Azure subscription ID | Future deployments and Azure administration. The prebuilt benchmark workflows do not need it. |
| Managed identity principal ID | Inspect Azure role assignments for access diagnostics. Do not use this as the client ID. |
| Blob endpoint | Connectivity diagnostics or Azure tools; no extra benchmark configuration field. |

Create the repository variables under **Settings → Secrets and variables → Actions
→ Variables**, not Secrets. Pass them to the prebuilt `history.yml@v1` and `pr.yml@v1`
workflows in your caller jobs:

```yaml
with:
  azure-client-id: ${{ vars.AZURE_CLIENT_ID }}
  azure-tenant-id: ${{ vars.AZURE_TENANT_ID }}
```

Grant each caller job `id-token: write` alongside its other benchmark-flow permissions.
The prebuilt workflows exchange OIDC tokens without a separate login action.
The [automation guide](https://folo-rs.github.io/folo/cargo-bench-history/github-automation.html)
provides complete caller examples.

Advanced CI jobs that invoke the CLI or root action directly set `AZURE_CLIENT_ID`
and `AZURE_TENANT_ID` in their job environment instead.

## Security model

One managed identity has account-scoped Storage Blob Data Contributor. It can
read/write/delete blobs and create/delete containers across the account, not only
the configured history container. An optional custom principal or current user
receives the same role independently.

A managed identity is an Azure principal, not a bench-history-specific identity type.
This deployment provisions it for the history account. A **federated credential** is
a trust rule attached to that identity: it allows a matching GitHub OIDC token to
authenticate as the identity. Its resource name describes its purpose, not an
additional access restriction.

`GithubOrg`, `GithubRepo` and `HistoryBranch` configure:

```text
repo:<GithubOrg>/<GithubRepo>:ref:refs/heads/<HistoryBranch>
repo:<GithubOrg>/<GithubRepo>:pull_request
```

The issuer is `https://token.actions.githubusercontent.com`; the audience is
`api://AzureADTokenExchange`. Branch trust is limited to the selected branch;
PR trust uses the repository's PR event context and is not limited to that target
branch. These subjects do not limit access to a particular workflow file or action.
`HistoryBranch` controls which workflow runs may authenticate, not which branches'
benchmark results may be stored or analyzed.
The `bench-history-default` credential serves default-branch collection, while
`HistoryBranch` explicitly selects the trusted branch. The
`bench-history-pull-request` credential trusts the PR context on the same identity.
Any job in a trusted context with effective `id-token: write` can use the identity;
neither credential checks that the job runs bench-history code.
See [GitHub's OIDC subject reference](https://docs.github.com/en/actions/reference/security/oidc#example-subject-claims).

Under GitHub's default permissions, fork PR jobs cannot obtain `id-token: write`,
including when the PR edits workflow YAML to request it. GitHub applies the
restriction after YAML permissions, and maintainer approval does not elevate it.
Without that permission, no OIDC token can be issued for Azure exchange.
An upstream PR subject, if issued, still names the upstream repository; that
subject is not a fork filter. A workflow in the fork's own repository has that
fork's subject and does not match the upstream subjects.

Keep the automation guide's same-repository job gate to skip unsupported fork work
explicitly, not as a substitute for the platform's token-issuance restriction.
Only grant `id-token: write` where needed. Code and actions running
with the identity are trusted with its storage rights; GitHub issue/comment rights
come separately from the job's `GITHUB_TOKEN`.
Fork PRs have no access to this history store; the reusable workflows skip them.

## Deployment behavior

Successful management-plane listings select creation only for missing storage.
Existing account, blob-service and container settings and data remain untouched.
New storage is private and Entra-only.

Deployment is incremental. Changing the custom principal adds a grant without
removing earlier grants; omission does not revoke access. Unmentioned resources
remain. Federation is configurable, not append-only: changing repository or
history-branch inputs updates the selected identity's existing credentials.
The child resources are named `bench-history-default` and `bench-history-pull-request`;
their names do not depend on the branch value. Different parent identities can use
the same child names. Use `ManagedIdentityName` and separate storage accounts to
distinguish production and test deployments, rather than a credential-name suffix.
Before deploying to an identity with differently named credentials, inspect any
matching issuer/subject and replace only the conflicting credential child. Preserve
the identity, role assignments and storage. Setup does not remove unmentioned children.
Serialize invocations targeting the same storage account or managed identity
in the selected subscription and resource group.

Failures remain failures, with child diagnostics. Already completed Azure changes
are not rolled back; resolve the failure and rerun with the intended inputs.
Direct Bicep users must set `createStorageAccount` and `createHistoryContainer`
to true only when the corresponding resource is absent. Routine deployments use
the driver to perform this discovery safely.
