# Synthetic benchmark output

The faker supplies explicitly chosen benchmark identities and measurements to
tests of benchmark-history collection, storage and analysis. It writes only the
engine output those consumers read. The library and executable use the same
writers so their output represents the same input values.

## Executable availability

In-workspace tests can locate a freshly prepared faker executable. A located
executable remains available across later faker builds, until its target tree is
explicitly cleaned. Each process caches its selected path.

A runner can supply an existing executable through `CBH_FAKER`, including a
relative path. The locator resolves that path to an absolute path before returning
it. The runner owns the supplied file and keeps it available while consumers use
it. A missing override causes the locator to build the faker.

The locator does not retry missing executable checks or weaken process-launch
coverage. Preparation failures retain build diagnostics.
