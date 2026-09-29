# Working on native execution

Follow the [application guidance](../cargo-release-plan/AGENTS.md) and the
[ownership guide](docs/implementation.md). Do not depend on publication or CLI models.
Never pass repository/upload credentials or controller toolchain overrides to build children.

Keep environment configuration in execution adapters, not pure argument tests.
Keep process-tree cancellation and owned cleanup intact on failures; retain both
worktree and directory cleanup causes. Run tests with `--all-features`.
