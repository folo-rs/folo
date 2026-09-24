# setup-azure

Use `setup-azure` once to prepare shared Azure storage for your repository, then
connect its printed settings to your benchmark configuration and GitHub workflows.
It creates or reuses a storage account, a history container and one managed identity
shared by collection, backfill and analysis. No checkout or benchmark configuration
is needed to run the command.

A **managed identity** is an Azure principal that receives access through role
assignments. A **federated credential** is a trust rule attached to that identity.
It lets GitHub Actions authenticate as the identity using short-lived OpenID Connect
(OIDC) tokens, without stored credentials. Azure checks the token's **subject**,
which identifies the repository and branch or pull-request context.

## Provision your repository

Choose an existing Azure subscription and GitHub repository, a resource-group name,
an Azure region and a globally unique storage-account name. Sign in to Azure CLI
in that subscription's tenant. Your account needs permission to create resources,
assign roles and configure managed-identity federation. Owner, or Contributor
combined with User Access Administrator on the relevant scope, are examples;
User Access Administrator alone cannot provision resources.

Sign in, select the subscription, then run setup with values for your deployment.
Every argument value below is customizable, not just the quoted placeholders:

```powershell
az login
az account set --subscription '<subscription-guid>'
cargo bench-history setup-azure `
    --subscription-id '<subscription-guid>' `
    --resource-group bench-history `
    --location westeurope `
    --storage-account '<unique-account-name>' `
    --github-owner firstnamelastname `
    --github-repository project `
    --history-branch main `
    --current-user
```

The command checks prerequisite software and the selected Azure login before any
Azure changes, and reports missing prerequisites as errors. It does not install
software or initiate login. The subscription is always explicit; the command does
not select your Azure CLI default.

The resource group, account, container and identity are created if needed. Existing
storage settings and history remain intact. The container defaults to `bench-history`;
the identity defaults to `id-<storage-account>-bench-history`. Use `--container` and
`--managed-identity` to choose different names, and use the existing resources' region
when deploying to them.
Separate accounts and `--managed-identity` names distinguish production and test
deployments; no credential-name suffix is needed.

### Give yourself or another principal access

`--current-user` grants the Azure CLI signed-in **user** account-scoped Storage Blob
Data Contributor access, in addition to the workflow identity.

Omit it if only workflows need access. To grant an existing user or group access
explicitly, replace it with:

```text
--custom-principal-id <entra-object-guid> --custom-principal-type user
```

Use `group` for an Entra group. Supply both flags and use an object ID from the
subscription's tenant, not an application/client ID. The custom principal is not
created by this command. `--current-user` conflicts with either custom-principal
flag.

## Connect the printed result

The command prints non-secret identifiers; it does not edit your repository or
GitHub settings. Make the following configuration changes:

| Output | What to do with it |
| --- | --- |
| `[storage.azure]` → `account` and `container` | Copy the printed TOML section into `.cargo/bench_history.toml` and commit it. These select where the commands read and write history. |
| Managed identity client ID | Create the GitHub Actions **repository variable** `AZURE_CLIENT_ID`. This selects the workflow identity for authentication. |
| Azure tenant ID | Create the repository variable `AZURE_TENANT_ID`. This selects the Entra tenant that authenticates the identity. |
| Azure subscription ID | Retain it for future deployments and Azure administration. The prebuilt benchmark workflows do not need it. |
| Managed identity principal ID | Use it to inspect the identity's Azure role assignments when diagnosing access. It is not the client ID and is not a benchmark workflow input. |
| Blob endpoint | Use it for connectivity diagnostics or Azure tools. There is no endpoint field to add to the standard benchmark configuration. |

Create the variables under **Settings → Secrets and variables → Actions → Variables**,
not under Secrets. Pass them to the prebuilt `history.yml` and `pr.yml`
workflows in your caller jobs:

```yaml
with:
  azure-client-id: ${{ vars.AZURE_CLIENT_ID }}
  azure-tenant-id: ${{ vars.AZURE_TENANT_ID }}
```

Grant each caller job `id-token: write` alongside the other permissions required
by its benchmark flow. This identity configuration applies to both action v1 and v2;
it does not select or upgrade the workflow revision. The
[GitHub automation guide](../github-automation.md) shows the complete caller examples
for v2. These workflows obtain their own short-lived tokens; a separate `azure/login`
step is not required.

Advanced CI jobs that invoke the CLI or root action directly set `AZURE_CLIENT_ID`
and `AZURE_TENANT_ID` in their job environment instead.

For commands on your workstation, the additional user/group grant lets your Azure
CLI login access the configured store. See [storage backends](../storage.md) for
runtime storage selection.

## Security and workflow scope

`--github-owner`, `--github-repository` and `--history-branch` configure these
default GitHub OIDC subjects on the shared identity:

```text
repo:<owner>/<repository>:ref:refs/heads/<history-branch>
repo:<owner>/<repository>:pull_request
```

`--history-branch` selects the branch whose GitHub workflow runs may authenticate.
It does not restrict which branches' benchmark results can be stored or analyzed:
a workflow running on `main` can backfill other commits. The PR subject
authorizes the repository's pull-request event context, not just PRs targeting that
branch. Neither subject restricts access to a particular workflow file or action.

The credential named `bench-history-default` serves default-branch history
collection; `--history-branch` explicitly selects its trusted branch. The credential
named `bench-history-pull-request` trusts the PR context on the same managed identity.
These names describe the credentials' roles, not additional OIDC restrictions.
The identity is provisioned for benchmark storage, but Azure does not restrict it
to bench-history code: any job in a trusted context with effective `id-token: write`
can authenticate as it.

The identity and any additional principal receive **Storage Blob Data Contributor
on the entire storage account**, not just the configured history container. This
permits reading, writing and deleting blobs and creating/deleting containers. Treat
code and actions running with that identity as trusted with those rights.
GitHub issue/comment permissions are separate: they use the workflow's
`GITHUB_TOKEN`, not the Azure identity.

With GitHub's default permissions, a fork PR job cannot obtain `id-token: write`,
even if the PR changes the workflow YAML to request it. GitHub applies that
restriction after reading the YAML, so the job cannot obtain an OIDC token to
exchange with Azure. Maintainer approval to run the workflow does not elevate
that permission. See GitHub's
[workflow-token permissions](https://docs.github.com/en/actions/security-for-github-actions/security-guides/automatic-token-authentication#permissions-for-the-github_token).

An upstream PR's OIDC subject, if a token is issued, identifies the upstream
repository even when the head comes from a fork. The Azure PR subject is therefore
not the fork filter; GitHub's default token-issuance permissions provide that
boundary. Workflows running in the fork's own repository identify that fork
instead and do not match this deployment's upstream subjects.

The [automation guide](../github-automation.md) also gates PR jobs to
`github.event.pull_request.head.repo.full_name == github.repository` so unsupported
fork work is skipped explicitly. Keep that gate, but do not treat an editable
workflow condition as the authorization boundary.

Fork PRs therefore have no access to this Azure store. The reusable workflows
provided by the GitHub automation do no work for fork PRs.

## Repeat deployments safely

Deployments are additive for storage and principal grants: another custom principal
gets an additional grant, while earlier grants, existing storage settings and history
remain. Omitting a principal does not revoke access.

Federation is configuration, not an append-only list. Changing the repository or
history branch updates the existing trust on the selected identity.
If an existing identity has a matching issuer/subject under a different credential
name, inspect and replace only that conflicting child before deployment. Preserve
the identity, its role assignments and storage; setup does not remove unmentioned
credential children.

Serialize invocations targeting the same storage account or managed identity.
Failures retain diagnostics, and completed Azure changes are not rolled back.
Rerun with the intended inputs after resolving the failure.

## Advanced: inspect or customize the deployment

`--out-dir <directory>` exports the self-contained deployment files instead of
deploying. For example:

```text
cargo bench-history setup-azure --out-dir ./azure-history --storage-account examplehistory --history-branch main
```

Supplied values prepopulate `parameters.json`; omitted required values remain for you
to fill in. Export does not start processes, probe software, contact Azure or require
login. `--current-user` therefore conflicts with `--out-dir`; after exporting, you
can run `deploy.ps1 -CurrentUser` when ready to authenticate and deploy.

The destination must be absent or empty; relative paths start at the invocation
directory. Review the files, fill the required parameters, and follow the exported
README's installation links and deployment instructions. The bundle needs no Rust
toolchain or Folo checkout, and includes no teardown command. Use `--help` for the
complete grouped CLI reference.
