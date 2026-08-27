//! Pseudoconsole PAL used on non-Windows builds.

use crate::pal::error::{PalError, PalErrorKind};
use crate::pal::ids::PtyId;
use crate::pal::pseudoconsole::{Pseudoconsole, WindowSize};

/// Stub pseudoconsole. The platform gate refuses to run before this is used.
#[derive(Debug, Default)]
pub(crate) struct BuildTargetPseudoconsole;

fn unsupported<T>() -> Result<T, PalError> {
    Err(PalError::new(PalErrorKind::Other))
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)]
impl Pseudoconsole for BuildTargetPseudoconsole {
    fn create(&self, _size: WindowSize) -> Result<PtyId, PalError> {
        unsupported()
    }

    fn resize(&self, _pty: PtyId, _size: WindowSize) -> Result<(), PalError> {
        unsupported()
    }

    fn write_input(&self, _pty: PtyId, _data: &[u8]) -> Result<(), PalError> {
        unsupported()
    }

    fn read_output(&self, _pty: PtyId) -> Result<Vec<u8>, PalError> {
        unsupported()
    }

    fn close(&self, _pty: PtyId) {}
}
