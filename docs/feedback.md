# Recurring review feedback

This chapter records the *kinds* of feedback that have repeatedly come up in human
review of this workspace. Its purpose is to let an automated reviewer (or an author
self-reviewing before requesting review) anticipate these objections and fix them
*before* a human has to point them out. Treat every item below as a question to ask
of your own diff.

Most of these are concrete instances of the meta-principles in the root `AGENTS.md`
("keep the house in order", "adjust patterns, fix entire classes of problems") and
of the topical chapters they cross-reference. When an item below overlaps a topical
chapter, that chapter is authoritative — this file is the *index of mistakes we
keep making*, not a replacement for the rules.

## Naming and terminology

* **Pick one term per concept and use it everywhere.** Reviewers react strongly to
  a single concept that goes by several names across the codebase (e.g. "engine"
  vs. "engine system" vs. "system"; "comparability key" vs. "discriminant set").
  Before introducing a noun for a domain concept, check what the rest of the code,
  the CLI surface, and the docs already call it, and converge on one. If the UI
  already names something, the code and docs should match the UI.
* **Singular vs. plural must be consistent.** When a type models a *set*, do not
  refer to the whole set in the singular by accident (a "discriminant set" is not
  "a discriminant").
* **Names should not encode provenance.** Do not bake *where an instance came from*
  into a type name (`ParsedKey` → `StorageKey`): name the thing for what it *is*,
  not how it was produced. "Parsed", "Resolved", "Computed" prefixes are usually
  noise.
* **Avoid catch-all module/file names.** `types.rs`, `misc.rs`, `utils.rs` and
  similar signal a failure to categorize. Almost every file contains types; put
  each item in a module named for its responsibility.

## Data model

* **Justify every `Option` field.** An optional field is a design statement ("this
  is sometimes absent"). If you cannot explain *why* it is absent in some cases,
  it probably should not be optional. Vague justifications ("so summaries lacking
  it degrade gracefully") are not enough.
* **Eliminate duplicative / redundant fields and types.** If two fields always
  imply each other (a metric's `name` and its `kind`; a `kind` and its `unit`),
  keep the one source of truth and derive the rest (`kind.as_unit()`). If two
  types model the same thing, collapse them; if one is genuinely a *query* over
  the other, its name should reflect that relation (`DiscriminantSet` vs.
  `DiscriminantSetQuery`, not an unrelated name like `Facets`).
* **Do not introduce a struct that is just a bag of two fields with no behavior.**
  If a wrapper (`struct Timestamps { commit, observation }`) adds no invariant or
  method of its own, fold its fields into the parent.
* **Offload format-specific shaping to format-specific code.** General-purpose
  types should not encode one engine's/format's arbitrary mapping (e.g. which
  benchmark-id component is the "group"). Let the engine-specific adapter build
  the general representation (e.g. a `Vec<String>` of segments).
* **Remove remnants of superseded designs.** Fields, flags, and concepts left over
  from an earlier iteration (an "effective timestamp" after the model moved to
  commit/observation times; an `analyze --timestamp` flag from a timestamp-based
  timeline) confuse readers. If you cannot articulate a current use case, remove it.

## Public API surface

* **Keep the public surface minimal.** Export only what callers (including `main.rs`
  and integration tests) actually use. A large `pub` surface forces neatness
  obligations (`#[non_exhaustive]`, doc completeness) onto types that were never
  meant for external consumption.
* **For a non-public-API crate, disable the public-API lints at crate level.** When
  a crate's surface exists only as a handoff boundary between `lib.rs` and
  `main.rs`/tests (not for external consumers), a crate-level `#![allow(...)]` with
  a `reason` is cleaner than scattering `#[non_exhaustive]` on every type.
* **Prefer `pub(crate)` for genuinely internal items.** Promote to `pub` only when
  something outside the crate truly needs it.

## Code style

* **Use `use` statements; avoid long inline paths.** `use` is free. Reserve
  fully-qualified `a::b::c::Item` paths for genuine disambiguation. Long paths
  sprinkled through the body for no reason are noise. (See `docs/code-style.md`.)
* **`expect()` is not a fancier `unwrap()`.** Follow `docs/error-handling.md`: an
  `expect()` message must explain *why the invariant holds*, not merely restate the
  operation. If you have nothing to say beyond "this should work", and `unwrap()`
  is permitted in that context, use `unwrap()`.
* **Do not overcomplicate the equivalent of `unwrap()` in test code.** An elaborate
  `match { … => panic!(...) }` that exists only to unwrap a value is harder to read
  than `.unwrap()`. In tests, just unwrap.

## Comments and documentation

* **Document the desired state, not history.** Inline and doc comments should
  describe how things *are*, never "replaces the old X", "previously was Y",
  "historical pattern". (See the user-preference memory on this.)
* **Do not document counts of things.** "the four callbacks", "three registration
  methods" — the count adds no value and churns whenever the count changes.
* **Cross-reference design docs by stable title, not chapter number.** Section
  numbers drift as documents are reorganized; `(see DESIGN §8.8)` rots. Reference
  the section *title* instead.
* **READMEs and examples must show realistic usage.** A walkthrough that produces
  no useful result (e.g. `analyze` immediately after a single `run`, with nothing
  to compare against) misleads. Show the realistic flow (e.g. seed history with
  `backfill` first).
* **No coward pluralization.** Never emit `run(s)` in user-facing/computed text;
  compute the correct singular/plural from the count. (See `docs/code-style.md`.)

## File and test organization

* **Follow Cargo conventions for binaries.** Binary crates live under `src/bin/`,
  not in ad-hoc locations.
* **Never use the `#[path]` attribute.** It is a sign of mis-structured files. To
  share test helpers, put them in a real package (the `testing` package or a
  dedicated crate), not a `#[path]`-included file.
* **Tests should be unit tests unless they have a real need to be integration
  tests.** CLI-argument parsing, pure mapping, and similar belong in in-module
  `#[cfg(test)]` unit tests. Reserve integration tests for things that must drive
  the binary from the outside. Mixing unit-test concerns into integration tests
  often forces extra `pub` exports (see "Public API surface").
* **Keep test files approachable.** A single enormous test file is unreadable even
  if a single binary is wanted for fixture reuse. Split into a `main.rs` plus
  topic submodules, and separate helper/harness code from the tests themselves.

## CLI / UX

* **Conflicting inputs are errors, not silent overrides.** If two flags cannot both
  apply (`--workspace` and `-p <pkg>`), reject the combination rather than letting
  one win — silent precedence hides mistakes.
* **Help text should say *when* to use a thing, not just *what* it does.** For a
  mode/flag, explain the situation it is for. State what relative paths resolve
  against. Spell out non-obvious consequences (e.g. that later blessings remain
  active past a context ref).
* **Keep CLI option groups consistently ordered across subcommands**, and put each
  option in the group that matches its role (a `--dry-run` is an execution flag,
  not a data-filter).
* **Question requirements that seem arbitrary.** If a constraint ("must be on the
  current branch") is not actually needed for correctness, drop it instead of
  documenting it. Reviewers will ask "why do we care?" — answer that *before* they
  ask.

## Process

* **Fix the whole class, not the one instance.** When a reviewer points at one
  occurrence, comb the codebase for every other occurrence and fix them all in the
  same pass. Expect the reviewer to be annoyed if the same issue recurs elsewhere.
