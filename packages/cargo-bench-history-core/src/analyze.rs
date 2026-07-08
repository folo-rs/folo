//! The analysis and rendering surface.
//!
//! Re-exported flat from the split `cbh_analysis` (timeline reconstruction and
//! change-point detectors) and `cbh_render` (terminal and Markdown rendering)
//! implementation crates, so consumers keep writing `analyze::Finding` and
//! `analyze::ReportFormat` against a single namespace.

pub use cbh_analysis::*;
pub use cbh_render::*;
