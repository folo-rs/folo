#[cfg(windows)]
use std::path::Component;
use std::path::Path;

use dunce::simplified;
use ohno::AppError;

use crate::action::errors::InvalidOutput;

pub(crate) fn for_output(path: &Path) -> Result<&str, AppError> {
    // Artifact upload interprets paths as Node glob patterns. Keep canonical paths for
    // native I/O and containment checks, simplifying only this handoff representation.
    // Dunce preserves extended paths when stripping the prefix could change their meaning.
    let path = simplified(path);
    #[cfg(windows)]
    if matches!(
        path.components().next(),
        Some(Component::Prefix(prefix)) if prefix.kind().is_verbatim()
    ) {
        return Err(InvalidOutput::new(
            "report path could not be converted to artifact-compatible Windows syntax; choose a standard temporary directory",
        )
        .into());
    }
    path.to_str()
        .filter(|value| !value.contains(['\r', '\n']))
        .ok_or_else(|| InvalidOutput::new("report paths must be single-line Unicode").into())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn output_paths_require_single_line_values() {
        for value in ["path\nreport.json", "path\rreport.json"] {
            for_output(Path::new(value)).unwrap_err();
        }
    }

    #[cfg(windows)]
    #[test]
    fn ordinary_drive_and_share_paths_preserve_their_spelling() {
        for value in [
            r"C:\Temp\Mixed Case\report.json",
            r"\\server\share\Mixed Case\report.json",
        ] {
            assert_eq!(for_output(Path::new(value)).unwrap(), value);
        }
    }

    #[cfg(windows)]
    #[test]
    fn safe_canonical_drive_paths_have_no_verbatim_prefix() {
        assert_eq!(
            for_output(Path::new(r"\\?\C:\Temp\Mixed Case\report.json")).unwrap(),
            r"C:\Temp\Mixed Case\report.json"
        );
    }

    #[cfg(windows)]
    #[test]
    fn unsupported_extended_paths_are_not_reinterpreted_as_ordinary_paths() {
        for value in [
            r"\\?\C:\Temp\NUL\report.json",
            r"\\?\C:\Temp\trailing.\report.json",
            r"\\?\C:\Temp\trailing \report.json",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1\report.json",
            r"\\?\UNC\server\share\report.json",
        ] {
            for_output(Path::new(value)).unwrap_err();
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_paths_are_unchanged() {
        for value in [
            "/tmp/Mixed Case/report.json",
            r"/tmp/\\?\literal/report.json",
        ] {
            assert_eq!(for_output(Path::new(value)).unwrap(), value);
        }
    }
}
