//! A scoped guard that restores the process working directory on drop.
//!
//! `#[serial]` tests that switch the process current directory share global
//! state, so a panic mid-test must not leak a changed directory into the next
//! test. Entering through this guard restores the previous directory in `Drop`,
//! which runs even while unwinding.

use std::path::{Path, PathBuf};

/// Switches the process current directory to `dir`, restoring the previous
/// directory when dropped.
#[derive(Debug)]
pub struct CwdGuard {
    original: PathBuf,
}

impl CwdGuard {
    /// Enters `dir`, remembering the current directory to restore on drop.
    #[must_use]
    pub fn enter(dir: &Path) -> Self {
        let original = std::env::current_dir().expect("current dir should be readable");
        std::env::set_current_dir(dir).expect("current dir should be set");
        Self { original }
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        let restored = std::env::set_current_dir(&self.original);
        // Restore failures are real problems, but panicking while already
        // unwinding would abort the process, so only assert when not panicking.
        if !std::thread::panicking() {
            restored.expect("current dir should be restored");
        }
    }
}
