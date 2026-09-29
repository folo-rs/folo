# Working on publication

Follow the [application guidance](../cargo-release-plan/AGENTS.md) and the
[ownership guide](docs/implementation.md). Do not perform live registry or GitHub writes
without explicit authorization; integration tests use owned local services.

Keep publication/batch/receipt identity and attempt selection here. Preserve native's
item deadline across upload and its completeness recheck. Run tests with
`--all-features`; candidate I/O scheduling must happen before its watchdog starts.
