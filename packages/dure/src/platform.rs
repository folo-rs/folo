//! Runtime platform gate.
//!
//! The crate compiles on every workspace target; only Windows may run the tool
//! (design.md, "Distribution").

use ohno::AppError;

use crate::UnsupportedPlatformError;

/// Returns whether this build can run `dure` commands other than `--help`.
#[must_use]
// Equivalent to the `cfg!(windows)` compile-time constant.
#[cfg_attr(test, mutants::skip)]
pub(crate) const fn is_supported_platform() -> bool {
    cfg!(windows)
}

/// Fails when the binary is not running on Windows.
// Trivial forwarder onto `ensure_supported`, which carries the tested logic. On
// Windows the failure branch is unreachable here, so replacing the body with
// `Ok(())` is an equivalent mutation.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn ensure_supported_platform() -> Result<(), AppError> {
    ensure_supported(is_supported_platform())
}

/// Fails when `supported` is false.
///
/// Split out from `ensure_supported_platform` so the failure branch is reachable
/// from tests on every platform, not just the platforms the tool refuses to run on.
fn ensure_supported(supported: bool) -> Result<(), AppError> {
    if supported {
        Ok(())
    } else {
        Err(UnsupportedPlatformError::new().into())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn support_matches_windows_cfg() {
        assert_eq!(is_supported_platform(), cfg!(windows));
    }

    #[test]
    fn unsupported_is_rejected() {
        let err = ensure_supported(false).unwrap_err();
        assert!(err.find_source::<UnsupportedPlatformError>().is_some());
    }

    #[test]
    fn supported_is_accepted() {
        ensure_supported(true).unwrap();
    }

    #[test]
    fn platform_gate_matches_this_platform() {
        assert_eq!(ensure_supported_platform().is_ok(), cfg!(windows));
    }
}
