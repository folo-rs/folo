#![expect(
    clippy::exhaustive_enums,
    clippy::exhaustive_structs,
    reason = "this crate is book infrastructure rather than a published API: every \
              consumer of its `pub` items lives in this workspace — its own binary, its \
              examples, and one integration test in cargo-bench-history. A new variant \
              or field here is an edit to the appendix, made in the same commit as the \
              call sites it affects, so reserving room to extend these types without \
              notice protects nobody and only costs those call sites exhaustive \
              construction and matching"
)]

//! Generates the registered figures, computed examples, configured values, and
//! serialized excerpts embedded in the `cargo-bench-history` book's "Data pipeline"
//! appendix.
//!
//! The appendix documents the tool's statistical processing end to end. Evidence that
//! materially depends on executable behaviour — gate thresholds, series values,
//! p-values, census tallies, report excerpts — is rendered from the same data the
//! appendix's tests assert against, written into
//! `packages/cargo-bench-history/book/src/appendix/generated/`, and included by the
//! book verbatim. Stable explanatory Markdown stays in the chapter files. A `--check`
//! run re-renders the generated assets into memory and compares, so a change in
//! behaviour that those assets describe fails the build instead of quietly making
//! the prose wrong.
//!
//! The crate is not published: it is book infrastructure, and it deliberately depends
//! on the `private-test-util` surface of `cbh_detect` so a figure and the test that
//! pins it read from one definition of the example data.
//! Its generator contract and ownership are described in
//! `packages/cargo-bench-history/docs/implementation.md` and
//! `packages/cargo-bench-history/docs/book.md`.
//!
//! # Layout
//!
//! * [`theme`] — the one palette, type scale and geometry every figure shares.
//! * [`canvas`] — SVG emission, including the light/dark theme adaptation and the
//!   determinism guarantees the `--check` run depends on.
//! * [`styles`] — the figure catalogue. Each style is a reusable primitive rather than
//!   a one-off drawing, so regenerating after a data change reproduces the same look.
//! * [`figures`] — the appendix's actual figures, where each module owns one chapter
//!   or a cohesive group of related chapters.
//! * [`glossary`] — the terms the appendix defines, feeding both the glossary page and
//!   each chapter's own term table.
//! * [`assets`] — the registry of everything the book embeds, and the write/check pair
//!   that keeps the checked-in copies honest.
//! * [`preview`] — a development-only page showing every figure on mdBook's default
//!   Light and Navy background/text-color pairs.

pub mod assets;
pub mod canvas;
pub mod figures;
pub mod glossary;
pub mod preview;
pub mod styles;
pub mod theme;
pub mod verdict;

mod coord;
