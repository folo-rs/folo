//! Local console PAL used on non-Windows builds.

use crate::pal::error::{PalError, PalErrorKind};
use crate::pal::local_console::LocalConsole;
use crate::pal::pseudoconsole::WindowSize;

/// Stub console. The platform gate refuses to run before this is used.
#[derive(Debug, Default)]
pub(crate) struct BuildTargetConsole;

fn unsupported<T>() -> Result<T, PalError> {
    Err(PalError::new(PalErrorKind::Other))
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)]
impl LocalConsole for BuildTargetConsole {
    fn has_console(&self) -> bool {
        false
    }

    fn stdin_is_terminal(&self) -> bool {
        false
    }

    fn disable_ctrl_c_handler(&self) -> Result<(), PalError> {
        unsupported()
    }

    fn enter_raw_relay(&self) -> Result<(), PalError> {
        unsupported()
    }

    fn window_size(&self) -> Result<WindowSize, PalError> {
        unsupported()
    }

    fn read_input(&self) -> Result<Vec<u8>, PalError> {
        unsupported()
    }

    fn write_output(&self, _data: &[u8]) -> Result<(), PalError> {
        unsupported()
    }

    fn read_prompt_line(&self) -> Result<String, PalError> {
        unsupported()
    }
}
