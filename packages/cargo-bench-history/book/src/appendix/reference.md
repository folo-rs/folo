# Reference tables

Key generated facts from the appendix, gathered for lookup. The owning chapters hold the
complete explanations; this page is a reference, not a substitute for the reasoning.

Every table here is generated from the code, so it cannot drift from the tool's behavior.

## Data

### What each engine measures

See [Shape of the data](shape.md#what-each-engine-measures).

{{#include generated/shape-engines.md}}

### How many series one benchmark yields

See [Shape of the data](shape.md#one-benchmark-is-not-one-number).

{{#include generated/shape-engine-series.md}}

### Which engines report dispersion

See [Shape of the data](shape.md#dispersion-what-the-engine-tells-you-about-its-own-precision).

{{#include generated/shape-dispersion.md}}

### How benchmark identity is formed

See [Shape of the data](shape.md#benchmark-identity).

{{#include generated/shape-identity.md}}

## Storage

### The stored run

See [Shape of the data](shape.md#what-a-stored-run-holds).

{{#include generated/shape-run.md}}

### Key grammar

See [Shape of the data](shape.md#where-it-all-lives).

{{#include generated/shape-key-grammar.md}}

### Object kinds

{{#include generated/shape-object-kinds.md}}

### Collision policies

See [Collection](collection.md#collisions-what-happens-when-a-run-already-exists).

{{#include generated/collection-conflicts.md}}

### What the machine key hashes

See [Collection](collection.md#target-triples-and-machine-keys).

{{#include generated/collection-machine-key.md}}

## Analysis

### Mode selection

See [Selection](selection.md#mode-is-derived-not-chosen).

{{#include generated/selection-mode-table.md}}

### Minimum evidence

See [Detection](detection.md#minimum-evidence).

{{#include generated/detection-minimums.md}}

### Every gate, per detector

See [Noise gates](gates.md#each-detector-has-its-own-sequence).

{{#include generated/gates-order.md}}

### Absolute floors

See [Noise gates](gates.md#gate-logic-is-the-move-big-enough-to-matter-relative_floor-absolute_floor).

{{#include generated/gates-floors.md}}

## Reporting

### Why a series was not judged

See [Multiplicity and coverage](coverage.md#what-a-report-judged).

{{#include generated/coverage-reasons.md}}

### Coverage states

See [Multiplicity and coverage](coverage.md#reading-a-silent-report).

{{#include generated/coverage-states.md}}

### Output formats

See [Reporting](reporting.md#the-formats). The JSON excerpt there is illustrative of
the report shape, not a complete field catalog.

{{#include generated/reporting-formats.md}}

### Comparison-base lag

See [Reporting](reporting.md#comparison-base-lag).

{{#include generated/reporting-lag.md}}

## Terms

Every term the appendix defines is in the [Glossary](glossary.md).
