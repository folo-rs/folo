//! The figures, tables and excerpts embedded in each appendix chapter.
//!
//! One module per chapter, so a chapter's evidence and its prose stay easy to keep in
//! step. A module exposes a single `assets` function; [`assets`] concatenates them.

use crate::assets::Asset;

pub mod glossary;

/// Every asset, from every chapter.
#[must_use]
pub fn assets() -> Vec<Asset> {
    glossary::assets()
}
