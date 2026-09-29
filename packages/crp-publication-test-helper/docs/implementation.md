# Publication credential fixture

This nonpublished helper connects Cargo to the production credential provider in
`crp_publication`'s local-registry boundary tests. The executable records each request
without changing it and delegates protocol handling and credential issuance to production.
The locator builds the executable in a dedicated child of the invoking test's target
directory. Only helper builds use that directory, avoiding the parent Cargo invocation's
build lock and executable-replacement races while retaining normal target cleanup.

The path-only development dependency makes the helper part of publication's test inputs
without shipping it or adding it to the application's version group. Its production-component
dependencies are also path-only. See the owning application's
[registry boundaries](../../cargo-release-plan/docs/implementation.md#registry-publication-boundaries)
and [test boundaries](../../cargo-release-plan/docs/implementation.md#test-boundaries).
