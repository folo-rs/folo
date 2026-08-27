//! Runtime platform gate.
//!
//! The crate compiles on every workspace target; only Windows may run the tool
//! (design.md, "Distribution").

use ohno::AppError;

use crate::UnsupportedPlatformError;

/// Returns whether this build can run `dure` commands other than `--help`.
#[must_use]
pub(crate) const fn is_supported_platform() -> bool {
    cfg!(windows)
}

/// Fails when the binary is not running on Windows.
pub(crate) fn ensure_supported_platform() -> Result<(), AppError> {
    if is_supported_platform() {
        Ok(())
    } else {
        Err(UnsupportedPlatformError::new().into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_matches_windows_cfg() {
        assert_eq!(is_supported_platform(), cfg!(windows));
    }

    #[cfg(not(windows))]
    #[test]
    fn refuses_to_run_off_windows() {
        let err = ensure_supported_platform().unwrap_err();
        assert!(ohno::ErrorExt::find_source::<UnsupportedPlatformError>(&err).is_some());
    }

    #[cfg(windows)]
    #[test]
    fn allows_windows() {
        ensure_supported_platform().unwrap();
    }
}
