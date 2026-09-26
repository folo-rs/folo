# Working on versioning

Follow the [application guidance](../cargo-release-plan/AGENTS.md) and the
[ownership guide](docs/implementation.md). Keep preparation, resolved state,
preview and application together; never resolve dependencies during application.

Run component tests with `--all-features`. Keep tests independent of the application,
publication and native packages. Exact error-condition assertions stay in this crate's
unit tests; boundary tests exercise outcomes, causes and filesystem effects.
