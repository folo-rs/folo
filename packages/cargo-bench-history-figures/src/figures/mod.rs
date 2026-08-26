//! The figures, tables and excerpts embedded in each appendix chapter.
//!
//! Each module owns one chapter or a cohesive group of related chapters, so the evidence
//! and its prose stay easy to keep in step. A module exposes a single `assets` function;
//! [`assets`] concatenates them.

use crate::assets::Asset;

pub mod coverage;
pub mod detection;
pub mod gates;
pub mod glossary;
pub mod pipeline;
pub mod reporting;
pub mod storage;

/// Every asset, from every chapter.
#[must_use]
pub fn assets() -> Vec<Asset> {
    let mut assets = glossary::assets();
    assets.extend(storage::assets());
    assets.extend(pipeline::assets());
    assets.extend(detection::assets());
    assets.extend(gates::assets());
    assets.extend(coverage::assets());
    assets.extend(reporting::assets());
    assets
}
