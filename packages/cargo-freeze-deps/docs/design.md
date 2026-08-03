# cargo-freeze-deps - Design

`cargo-freeze-deps` turns floating Cargo dependency requirements into reproducible exact
requirements while preserving the surrounding manifest as authored. It is intentionally a
manifest transformation rather than a dependency resolver: it derives a literal version from
the stated requirement and does not consult a registry.

## Transformation contract

All standard package, target-specific, and workspace dependency tables participate.
Dependencies without a version remain untouched, as do patch and replacement tables. A
supported requirement is frozen to an exact, fully specified form of its comparator's own
matching version literal, zero-filling omitted minor or patch components. For example,
`<=1.2.3` becomes `=1.2.3`. Requirements that cannot be reduced without discarding constraints
or consulting available releases remain unchanged.

Comments and document layout are preserved because the manifest is edited structurally rather
than regenerated. The complete input is parsed and transformed before output begins, so
malformed TOML, malformed version fields, and invalid supported requirements fail without
writing a partially transformed document. The caller chooses whether the completed document
replaces the input or is written elsewhere.

## Diagnostics and error boundary

Success reports how many dependency requirements were frozen and how many were skipped because
their forms are unsupported. Requirements already in the target exact form contribute to
neither count. Failures identify the operation or dependency involved; file access and parsing
failures also identify the relevant path. The CLI emits diagnostics on stderr and produces a
failure status.

Failure diagnostics retain parser, semantic-version, and filesystem causes and identify the
relevant dependency or path. Underlying errors are not reduced to strings or obscured by a
generic category.
