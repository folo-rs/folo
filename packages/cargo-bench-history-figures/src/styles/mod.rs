//! The figure catalogue: one module per reusable figure style.
//!
//! A style is a *primitive*, not a drawing. When the data behind the appendix changes,
//! every figure is re-rendered by re-running the generator, and each comes back in the
//! same visual language it had before — which is only true because no figure is drawn
//! ad hoc.
//!
//! Most data figures compose [`plot::Plot`]; the styles that do not (a gate ladder, a
//! stacked census bar) are shapes a value-against-position plot cannot express.

pub mod gates;
pub mod ladder;
pub mod multiplicity;
pub mod occupancy;
pub mod operation;
pub mod plot;
