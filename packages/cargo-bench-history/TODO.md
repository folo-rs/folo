# Follow-up work

## Benchmark fork-origin pull requests

Fork benchmarking is blocked until the product supports secure federated access to the target
branch's benchmark history. Define and validate the authorization and workflow model for that
access before enabling collection, comparison or publication for fork-origin heads.

The repository-level `pull_request` OIDC subject identifies a PR workflow context, not whether
the PR head belongs to a fork. It is not a fork-head filter. Keep the explicit same-repository
product gate until the supported federation model is established; do not substitute a PAT,
static client secret, storage key or other long-lived credential fallback.

References: [GitHub OIDC claims](https://docs.github.com/en/actions/reference/security/oidc)
and [Microsoft Entra workload identity federation](https://learn.microsoft.com/en-us/entra/workload-id/workload-identity-federation).
