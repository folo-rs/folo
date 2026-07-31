# Design documentation

How user-visible behavior and its design tenets are captured across the workspace:
which package owns them, where they live, and how to keep them current.

## Where design documentation lives

* Give each package that owns an independent user-visible behavior contract a
  package-level `docs/design.md`. This includes applications and independently
  consumable libraries.
* A private implementation crate that only implements an owning application and has
  no independent behavior contract does not need a separate design document. Its
  behavioral design belongs in the owning application's design document. Behavioral
  component documents, when useful, live under the owning application's `docs/` and
  are linked from its `design.md`.
* When an owning package has multiple significant behavioral components, give each
  its own document under `docs/`, referenced from that package's `design.md`.
* Keep the owning design documentation current whenever user-visible behavior or its
  design tenets change.

## What a design document contains

Keep it a behavioral design document, not an implementation record: describe the
user-visible behavior, its design tenets, and the relationships between behavioral
concepts — *what* users can rely on and *why*.

* Keep internal architecture, ownership boundaries, and non-user-visible tenets in
  the package's [implementation guide](implementation.md).
* Do not name private types, methods, fields, or flags, and do not transcribe control
  flow or data structures. Illustrate behavior with examples rather than catalogues
  or listings of types and functions.
* Describe the desired end state as the final achieved state. Do not write
  "in progress" language, changelogs, or historical notes.
* Do not maintain a running or numbered decision log. Recording the alternatives
  that were considered and why they were not chosen is fine when it explains the
  design; a chronological ledger of changes is not.
