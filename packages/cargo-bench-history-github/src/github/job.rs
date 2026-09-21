use std::num::NonZero;

use serde::Deserialize;

/// Run-bound job facts used to reconcile collection attempts, not analysis commits.
///
/// GitHub may report a merge-ref head SHA for a PR job. The receipt, not this
/// representation, binds the explicitly frozen commit passed to the analyzer.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct WorkflowJob {
    pub(crate) id: NonZero<u64>,
    pub(crate) run_id: NonZero<u64>,
    pub(crate) run_attempt: NonZero<u64>,
    pub(crate) name: String,
    pub(crate) status: String,
    pub(crate) conclusion: Option<String>,
}
