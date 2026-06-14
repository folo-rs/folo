# API documentation

This chapter covers the conventions for writing `///` and `//!` documentation
comments, README files, and rendered API documentation artifacts (diagrams,
feature gates, intra-doc links).

## Document the contract, not the implementation

API documentation on types and functions should describe the API contract (i.e.
the inputs, the outputs and the behavior), not how it is implemented. Do not
discuss implementation details like private helper types or reference the internal
field structure of a type in API documentation.

Specific things that are implementation details and do **not** belong in API
documentation:

* Internal data structures (e.g. "uses an intrusive linked list").
* Internal ordering guarantees (e.g. "FIFO") unless they are part of the API
  contract.
* What fields a node or struct contains internally.
* Whether a future is `Unpin` or not (obvious from the type's auto-trait
  inference).

## Keep API documentation summary lines short

The first line of each `///` doc comment is used as the summary in rustdoc type
and method tables. Keep this first line short and concise so it does not wrap to
the next line in the generated documentation. Move detailed descriptions to
subsequent paragraphs after a blank `///` line.

## Documentation is about today, not about yesterday

Both inline and API documentation must describe the current facts, not history
from previous designs or iterations. Do not make comparisons with mechanisms that
no longer exist in the API. Do not discuss how we got to the current design. Do
not discuss what is in progress and what is completed.

## Hide async entrypoint in examples

In inline code examples that use `.await`, do not render the async
entrypoint/wrapper in generated API documentation.

Example:

```rust
use events::once::Event;
# use futures::executor::block_on;

# block_on(async {
let event = get_event();
let message = event.await;

assert_eq!(message, "Hello, World!");
# });
```

## Diagrams

Mermaid diagrams are encouraged in API documentation where they make sense.

Each diagram should be a separate file in a `package-name/docs/diagrams` folder, with the
file extension `.mermaid`. The file name should match the diagram name used in the
documentation.

Rendering Mermaid diagrams in Rust API documentation requires the `simple-mermaid`
package to be referenced. The syntax for embedding a Mermaid diagram is
`#![doc = mermaid!("../doc/region_cached.mermaid")]`. See existing examples for a
detailed reference (e.g. the `region_local` package).

## Examples for README.md files

In each package with a `README.md` file, there should be a corresponding
`examples/package_name_readme.rs` file that contains the Rust code present in the
example. This is important to verify that the example actually works.

If the two are out of sync, use the `package_name_readme.rs` as the authoritative
source and update the `README.md` file to match it.

It is fine to disable Clippy rules in the `package_name_readme.rs` file, as it is
not production code and often needs to take shortcuts to be short and simple.

If the package name is long, use an abbreviation (e.g. `foo_bar_baz/examples/fbb_readme.rs`).

## Feature-gated documentation

Use `#![cfg_attr(docsrs, feature(doc_cfg))]` at the crate root. The `doc_cfg`
feature now includes automatic inference of feature gates from
`#[cfg(feature = "...")]` attributes, so per-item
`#[cfg_attr(docsrs, doc(cfg(...)))]` annotations are not needed.

Do not use `doc_auto_cfg` — it has been removed and merged into `doc_cfg`.

Ensure `Cargo.toml` contains:

```toml
[package.metadata.docs.rs]
all-features = true
```

`doc(cfg(...))` (via the `doc_cfg` feature) controls the "available on feature X"
*badge* rustdoc renders next to an item; it does not make an intra-doc link
conditional. For the link-side of the problem, see the next section.

## Intra-doc links to feature-gated items

### The problem

An item — a module, type, trait, function, or trait `impl` — may only exist
under a non-default cfg, for example:

```rust
#[cfg(any(test, feature = "test-util"))]
pub mod fake;

#[cfg(any(test, feature = "futures-stream"))]
impl<T> futures_core::Stream for FutureDeque<T> { /* ... */ }
```

If a doc comment contains an intra-doc link to such an item, the link resolves
only when the gating feature is enabled. Building the documentation **without**
that feature (which is what happens by default, and what the
`docs-default-features` validation step does) then fails with:

```text
error: unresolved link to `futures_core::Stream`
   = note: `-D rustdoc::broken-intra-doc-links` implied by `-D warnings`
```

This is easy to miss locally, because `validate-local`'s main docs build and
docs.rs both use `--all-features`, where the linked item *does* exist. The
dedicated `docs-default-features` check exists specifically to catch this class
of error.

### The anti-patterns

1. **Unconditional link to a gated item.** A plain
   ``[`Stream`][futures_core::Stream]`` in an ungated doc comment. Builds fine
   with `--all-features`, fails with default features.

2. **Dropping the link to dodge the warning.** Writing `` `Stream` `` (no link),
   or pasting a full `https://docs.rs/...` URL instead of an intra-doc link,
   purely to avoid the broken-link error. This silently degrades the
   documentation for users who *do* enable the feature — they lose the
   clickable, version-correct link rustdoc would otherwise generate.

### The fix: feature-conditional doc lines

Split the affected doc line into two `cfg_attr` doc attributes — one that emits
the proper intra-doc link when the gating cfg is active, and a plain-text
fallback otherwise. Use `#![...]` (inner) for module/crate-level `//!` docs and
`#[...]` (outer) for item-level `///` docs.

Crate/module-level (`//!`) example:

```rust
#![cfg_attr(
    any(test, feature = "test-util"),
    doc = "See the [`fake`] module for detailed examples and API documentation."
)]
#![cfg_attr(
    not(any(test, feature = "test-util")),
    doc = "See the `fake` module (available with the `test-util` feature) for detailed examples and API documentation."
)]
```

Item-level (`///`) example, interleaved with the surrounding doc block:

```rust
/// `Poll::Pending` when the respective end is not yet ready.
///
#[cfg_attr(
    any(test, feature = "futures-stream"),
    doc = "With the `futures-stream` feature, `FutureDeque` also implements [`Stream`][futures_core::Stream], yielding completed results from the front."
)]
#[cfg_attr(
    not(any(test, feature = "futures-stream")),
    doc = "With the `futures-stream` feature, `FutureDeque` also implements `Stream`, yielding completed results from the front."
)]
///
/// # Examples
```

Rules for applying the pattern:

- **The cfg condition must exactly match the gate on the linked item.** If the
  item is `#[cfg(any(test, feature = "foo"))]`, the link branch uses
  `any(test, feature = "foo")` and the fallback uses
  `not(any(test, feature = "foo"))`. Including `test` keeps the link live in
  test builds (where the item also exists).
- **The fallback must carry the same information**, minus the broken link.
  Mention the feature by name so a reader on default features still understands
  the capability exists and how to enable it.
- **`#[doc = "..."]` is exactly equivalent to a `///` line.** You can freely
  interleave `#[cfg_attr(..., doc = "...")]` attributes between `///` lines;
  rustdoc renders them in source order. Keep a blank `///` line on either side
  if the surrounding text is separate paragraphs.
- **Long `doc = "..."` strings are acceptable here.** rustfmt does not reflow
  string literals, so a single-line doc string that exceeds the usual 100-column
  guideline is fine — it cannot be wrapped without changing the rendered output.
