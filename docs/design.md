# Design documentation

How design intent is captured across the workspace: where it lives, what it should
contain, and how to keep it current.

## Where design documentation lives

* Record design elements, key decisions, and architectural choices as inline `//`
  comments (not `///` API-documentation comments) in the files to which they apply.
* Give each package a package-level `package-name/docs/design.md`. All major
  refactoring or redesign work must be accompanied by design documentation, kept up
  to date as the design changes.
* When a package has multiple significant sub-components, give each its own
  `package-name/docs/componentN.md`, referenced from that package's `design.md`.

## What a design document contains

Keep it a design document, not an implementation record: describe patterns, tenets,
and relationships — *what* exists and *why*, never *how* it is coded.

* Stay high-level. Do not name private types, methods, fields, or flags, and do not
  transcribe control flow, data structures, or wire-protocol mechanics. Illustrate
  with examples rather than catalogues or listings of types and functions.
* Spell out implementation detail only for a genuinely exceptional corner case that
  cannot be understood otherwise. When in doubt, cut detail — the code and its tests
  are the record of *how*.
* Describe the desired end state as the final achieved state. Do not write
  "in progress" language, changelogs, or historical notes. Open questions are fine.
* Do not maintain a running or numbered decision log. Recording the alternatives
  that were considered and why they were not chosen is fine when it explains the
  design; a chronological ledger of changes is not.
