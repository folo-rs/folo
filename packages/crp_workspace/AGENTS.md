# Working on workspace observations

Follow the [application guidance](../cargo-release-plan/AGENTS.md) and the
[ownership guide](docs/implementation.md). Keep Git access subprocess-based and
preserve raw manifest syntax through exact-dependency validation.

Run component tests with `--all-features` so shared private fixtures are available.
Unit fixtures must provide captured directory identities through injected callbacks;
never canonicalize fixture paths on the host. Real canonicalization and path-alias
cases belong in boundary tests. Do not import versioning group types.
