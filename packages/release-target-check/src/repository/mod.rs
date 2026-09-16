pub use snapshot::Repository;
pub(crate) use snapshot::{VerificationError, canonicalize};

mod snapshot;
mod validation;
