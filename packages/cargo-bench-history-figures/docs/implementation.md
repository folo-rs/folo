# Book evidence generation

This unpublished package owns the asset registry, deterministic rendering, reconciliation
and preview for the `cargo-bench-history` user guide. The owning application's
[book architecture](../../cargo-bench-history/docs/book.md) defines the reader journeys.

Statistical fixtures and production projections feed generated content; explanatory
prose stays in the book. Worked reports reuse detector-owned example series and the
core renderer. Workflow cards consume those reports through the companion's offline
JSON projection with an in-memory evidence reader. Illustrative invocation facts never
claim an actual benchmark execution, repository topology or GitHub publication.

The registry includes downloadable full reports as well as embedded summaries and
figures. Markdown downloads use `.markdown` to avoid mdBook's chapter-link rewriting.
The write/check pair owns their freshness and removes unregistered generated files.
No supported import or synthetic-engine command is part of the user walkthrough.

## Accessible figures

Styles supply titles and data-derived descriptions at the SVG boundary. The boundary
escapes metadata and supplies `role="img"` with an accessible name. Names use no global
IDs, because mdBook's print view and the theme preview can include the same figure more
than once.

Text inherits the reader's theme color. Data roles also use shapes, symbols or numbered
labels so color is supplementary. The book's keyboard-focusable figure containers
scroll locally at narrow widths, retaining the authored minimum width rather than
shrinking labels. Keep that minimum synchronized with `theme::WIDTH`.

Pure metadata and projection tests remain in-process. Full SVG/statistical generation
tests retain justified native-only coverage where Miri would add no memory-safety signal.
The preview shows both Light and Navy renderings. Verify the built book as well:
includes, links, expandable report evidence and surrounding captions are page behavior,
not properties of an isolated SVG.
