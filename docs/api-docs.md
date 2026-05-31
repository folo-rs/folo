# API documentation conventions

This document collects conventions for writing API documentation (`///` and `//!` comments)
that are easy to get subtly wrong and that our tooling enforces.

## Intra-doc links to feature-gated items

### The problem

An item — a module, type, trait, function, or trait `impl` — may only exist under a non-default
cfg, for example:

```rust
#[cfg(any(test, feature = "test-util"))]
pub mod fake;

#[cfg(any(test, feature = "futures-stream"))]
impl<T> futures_core::Stream for FutureDeque<T> { /* ... */ }
```

If a doc comment contains an intra-doc link to such an item, the link resolves only when the
gating feature is enabled. Building the documentation **without** that feature (which is what
happens by default, and what the `docs-default-features` validation step does) then fails with:

```text
error: unresolved link to `futures_core::Stream`
   = note: `-D rustdoc::broken-intra-doc-links` implied by `-D warnings`
```

This is easy to miss locally, because `validate-local`'s main docs build and docs.rs both use
`--all-features`, where the linked item *does* exist. The dedicated `docs-default-features`
check exists specifically to catch this class of error.

### The anti-patterns

1. **Unconditional link to a gated item.** A plain `[\`Stream\`][futures_core::Stream]` in an
   ungated doc comment. Builds fine with `--all-features`, fails with default features.

2. **Dropping the link to dodge the warning.** Writing `` `Stream` `` (no link), or pasting a
   full `https://docs.rs/...` URL instead of an intra-doc link, purely to avoid the broken-link
   error. This silently degrades the documentation for users who *do* enable the feature — they
   lose the clickable, version-correct link rustdoc would otherwise generate.

### The fix: feature-conditional doc lines

Split the affected doc line into two `cfg_attr` doc attributes — one that emits the proper
intra-doc link when the gating cfg is active, and a plain-text fallback otherwise. Use `#![...]`
(inner) for module/crate-level `//!` docs and `#[...]` (outer) for item-level `///` docs.

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

- **The cfg condition must exactly match the gate on the linked item.** If the item is
  `#[cfg(any(test, feature = "foo"))]`, the link branch uses `any(test, feature = "foo")` and the
  fallback uses `not(any(test, feature = "foo"))`. Including `test` keeps the link live in test
  builds (where the item also exists).
- **The fallback must carry the same information**, minus the broken link. Mention the feature by
  name so a reader on default features still understands the capability exists and how to enable
  it.
- **`#[doc = "..."]` is exactly equivalent to a `///` line.** You can freely interleave
  `#[cfg_attr(..., doc = "...")]` attributes between `///` lines; rustdoc renders them in source
  order. Keep a blank `///` line on either side if the surrounding text is separate paragraphs.
- **Long `doc = "..."` strings are acceptable here.** rustfmt does not reflow string literals, so
  a single-line doc string that exceeds the usual 100-column guideline is fine — it cannot be
  wrapped without changing the rendered output.

### Why not `#[doc(cfg(...))]`?

`doc(cfg(...))` (via the `doc_cfg` feature) controls the "available on feature X" *badge* rustdoc
renders next to an item; it does not make an intra-doc link conditional. The two mechanisms are
complementary: `doc_cfg` is configured once at the crate root (see the "Feature-gated
documentation" section of [AGENTS.md](../AGENTS.md)) and annotates gated *items*, whereas the
feature-conditional doc-line pattern above fixes *links* from ungated docs to gated items.
