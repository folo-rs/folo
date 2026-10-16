# Follow-up work

## Decide metabench interoperability

Defer interoperability until a product decision selects direct support or consumption
of a suitable emitted dataset, potentially with changes to that dataset's producer.
Neither option is committed support. Current integration guidance covers existing engines only.

## Benchmark fork-origin pull requests

Fork benchmarking is not supported. A possible extension is secure federated access to the
target branch's benchmark history. It requires a defined and validated authorization and
workflow model before collection, comparison or publication for fork-origin heads can be enabled.

The repository-level `pull_request` OIDC subject identifies a PR workflow context, not whether
the PR head belongs to a fork. It is not a fork-head filter. Preserve the explicit exclusion of
fork-origin work in the current product; do not substitute a PAT,
static client secret, storage key or other long-lived credential fallback.

References: [GitHub OIDC claims](https://docs.github.com/en/actions/reference/security/oidc)
and [Microsoft Entra workload identity federation](https://learn.microsoft.com/en-us/entra/workload-id/workload-identity-federation).
