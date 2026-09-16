# Stress harness design

## Purpose

The harness measures the real `cargo-bench-history` analysis path over an invented,
reproducible dataset. It does not change production analysis or require real
benchmark collection. The [README](../README.md) describes the dataset, backends,
command-line options and report.

## Execution

A scenario requires benchmarks and first-parent main commits. Feature-branch
commits and dirty snapshots may be absent. Invalid scenario sizes are rejected
before creating repository or storage resources.

The analysis workspace, synthetic Git repository and storage root are separate:
configuration and import bookkeeping must not make the measured repository dirty.
The workspace configuration identifies the seeded project and, for cloud storage,
its account and container. Local storage is selected through the analysis invocation
rather than embedded in the configuration.

Provisioned storage is cleaned up after measurement, including when seeding, upload
or analysis fails. Explicit retention preserves the data. An execution failure
takes precedence over a simultaneous cleanup failure; cleanup failures are otherwise
reported. Successful execution returns a successful process status; failures are
reported on stderr and return a failing status.

## Reproducibility

Dataset generation and the analysis clock depend on the scenario rather than the
current date. Integration scenarios verify the requested modes and seeded findings,
not elapsed-time thresholds. Runtime measurements are observations for manual
scaling experiments, not test assertions.
