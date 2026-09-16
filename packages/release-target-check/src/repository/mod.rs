pub use snapshot::Repository;
pub(crate) use snapshot::{VerificationError, canonicalize};
pub(crate) use validation::validate_around;

mod snapshot;
mod validation;
