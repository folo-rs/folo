pub use decision::DECISION_SCHEMA_VERSION;
pub(crate) use decision::SemanticImpact;
pub use generate::run_propose;

mod alignment;
mod decision;
mod generate;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
