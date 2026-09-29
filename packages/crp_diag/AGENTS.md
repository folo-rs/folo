# Working on diagnostics

Follow the [ownership guide](docs/implementation.md). Keep this component dependency-light
and limited to reporting and presentation. Do not add release models or filesystem helpers.

Test with in-memory/failing destinations. Disabled verbose notes must not evaluate
formatting closures; closed destinations must not make advisory notes abort an operation.
