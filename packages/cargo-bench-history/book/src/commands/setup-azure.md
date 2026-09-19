# setup-azure

Use `setup-azure` once to prepare shared Azure history for your repository, then
connect its printed settings to your benchmark configuration and GitHub workflows.
It creates or reuses a storage account, a history container and one managed identity
shared by collection, backfill and analysis. No checkout or benchmark configuration
is needed to run the command.

A **managed identity** is an Azure principal that receives access through role
assignments. GitHub Actions authenticates as it using short-lived OpenID Connect
(OIDC) tokens, without stored credentials. Azure checks the token's **subject**,
which identifies the repository and branch or pull-request context.

## Provision your repository

Choose an existing Azure subscription and GitHub repository, a resource-group name,
an Azure region and a globally unique storage-account name. Sign in to Azure CLI
in that subscription's tenant. Your account needs permission to create resources,
assign roles and configure managed-identity federation. Owner, or Contributor
combined with User Access Administrator on the relevant scope, are examples;
User Access Administrator alone cannot provision resources.

Replace the quoted placeholders, and set your repository owner, name and history
branch:

```powershell
cargo bench-history setup-azure `
    --subscription-id '<subscription-guid>' `
    --resource-group team-history `
    --location westeurope `
    --storage-account '<unique-account-name>' `
    --github-owner example `
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

### Give yourself or another principal access

`--current-user` grants the Azure CLI signed-in **user** account-scoped Storage Blob
Data Contributor access, in addition to the workflow identity. It resolves the user's
object ID in the selected subscription's tenant, including a guest user's object in
that tenant, before changing Azure resources. A service-principal login cannot use
this shortcut; the user lookup also requires Microsoft Graph access. Azure CLI's
directory lookup uses its active subscription: if that differs from the explicit
target, the command asks you to run `az account set --subscription <subscription-guid>`
and retry. It does not change that default itself.

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
| Azure subscription ID | Retain it for future deployments and Azure administration. Direct benchmark OIDC does not need it. If you use `azure/login` separately, pass it as `subscription-id`. |
| Managed identity principal ID | Use it to inspect the identity's Azure role assignments when diagnosing access. It is not the client ID and is not a benchmark workflow input. |
| Blob endpoint | Use it for connectivity diagnostics or Azure tools. There is no endpoint field to add to the standard benchmark configuration. |

Create the variables under **Settings → Secrets and variables → Actions → Variables**,
not under Secrets. In the job that runs the root benchmark action or invokes
`cargo bench-history` directly, set:

```yaml
env:
  AZURE_CLIENT_ID: ${{ vars.AZURE_CLIENT_ID }}
  AZURE_TENANT_ID: ${{ vars.AZURE_TENANT_ID }}
```

Grant that job `id-token: write` alongside the other permissions required
by its benchmark flow. The [GitHub automation guide](../github-automation.md)
shows complete caller workflows using the root composite action. Its steps inherit
the job environment. The tool obtains its own short-lived token; an `azure/login`
step is not required for that path.

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

The branch subject authorizes runs on the selected history branch. The PR subject
authorizes the repository's pull-request event context, not just PRs targeting that
branch. Neither subject restricts access to a particular workflow file or action.
The issuer is `https://token.actions.githubusercontent.com`; the audience is
`api://AzureADTokenExchange`. Jobs using an environment or customized subject format
need corresponding trust configuration rather than these default subjects.

The identity and any additional principal receive **Storage Blob Data Contributor
on the entire storage account**, not just the configured history container. This
permits reading, writing and deleting blobs and creating/deleting containers. Treat
code and actions running with that identity as trusted with those rights.
GitHub issue/comment permissions are separate: they use the workflow's
`GITHUB_TOKEN`, not the Azure identity.

The PR workflow in the [automation guide](../github-automation.md)
**skips fork PRs before credentialed work**; retain its same-repository gate.
Azure's `pull_request` subject itself does not distinguish fork heads.
For a custom PR workflow, gate the credentialed job with
`github.event.pull_request.head.repo.full_name == github.repository` and grant
`id-token: write` only where needed. Provisioning federation does not add this gate
to your workflows. See GitHub's
[OIDC subject reference](https://docs.github.com/en/actions/reference/security/oidc#example-subject-claims).

Fork benchmarking is unsupported until it can securely obtain federated access to
the base repository's history. Stored credentials are not a substitute.

## Repeat deployments safely

Deployments are additive for storage and principal grants: another custom principal
gets an additional grant, while earlier grants, existing storage settings and history
remain. Omitting a principal does not revoke access.

Federation is configuration, not an append-only list. Changing the repository or
history branch updates the existing trust on the selected identity. In particular,
the stable credential resource `github-branch-main` follows `--history-branch`
even when that branch is not `main`; changing the branch replaces its subject.

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
