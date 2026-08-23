# Implementation documentation

How internal architecture is captured across the workspace: where implementation
guides live, how package boundaries are connected, and what they should contain.

## Where implementation documentation lives

* Give every package a package-level `docs/implementation.md`, including
  applications, independently consumable libraries, and private implementation
  crates.
* When an application is implemented by private crates, its implementation guide
  links the package guides and explains how their ownership boundaries form one
  coherent application architecture.
* When a package has multiple significant implementation components, give each its
  own document under `docs/`, referenced from that package's `implementation.md`.
* Keep implementation documentation current whenever internal architecture,
  ownership boundaries, or implementation tenets change.

## What an implementation guide contains

Describe internal architecture, ownership boundaries, and non-user-visible
implementation tenets — the structure that keeps the package coherent and why that
structure exists.

* Keep user-visible behavior and its design tenets in the owning package's
  [design document](design.md). A private implementation crate with no independent
  behavior contract refers to its owning package's design documentation instead of
  restating that behavior.
* Stay high-level. Do not catalogue private types, methods, fields, or flags, and do
  not transcribe control flow or data structures. Use links to component guides when
  an area needs more detail.
* Describe the desired end state as the final achieved state. Do not write
  "in progress" language, changelogs, or historical notes.
* Do not maintain a running or numbered decision log. Recording alternatives and
  their trade-offs is useful when it explains the architecture; a chronological
  ledger of changes is not.
