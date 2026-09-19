use std::path::Path;

use dunce::simplified;
use ohno::AppError;

use crate::action::errors::InvalidOutput;

pub(crate) fn for_output(path: &Path) -> Result<&str, AppError> {
    // Artifact upload interprets paths as Node glob patterns. Keep canonical paths for
    // native I/O and containment checks, simplifying only this handoff representation.
    // Dunce preserves extended paths when stripping the prefix could change their meaning.
    let path = simplified(path);
    let value = path
        .to_str()
        .filter(|value| !value.contains(['\r', '\n']))
        .ok_or_else(|| InvalidOutput::new("report paths must be single-line Unicode"))?;
    // These outputs also name literal files for report readers, so returning escaped glob
    // patterns would change their meaning. Reserve wildcard/class syntax instead.
    // @actions/glob disables brace expansion and extended operators; those remain literal.
    if value.contains(['*', '?', '[']) {
        return Err(InvalidOutput::new(
            "report paths must not contain artifact wildcard or character-class syntax; choose a literal temporary directory",
        )
        .into());
    }
    #[cfg(not(windows))]
    if value.contains('\\') {
        return Err(InvalidOutput::new(
            "report paths must not contain artifact glob escape characters; choose a literal temporary directory",
        )
        .into());
    }
    Ok(value)
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

    #[test]
    fn artifact_pattern_syntax_is_rejected() {
        for directory in ["wild*card", "wild?card", "class[ab]", "escaped[[]"] {
            for_output(&Path::new("reports").join(directory).join("report.json")).unwrap_err();
        }
    }

    #[test]
    fn braces_closing_brackets_and_parentheses_remain_literal() {
        for directory in [
            "literal{a,b}",
            "literal]",
            "literal+(name)",
            "literal!#name",
        ] {
            let path = Path::new("reports").join(directory).join("report.json");
            assert_eq!(for_output(&path).unwrap(), path.to_str().unwrap());
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
        for value in ["/tmp/Mixed Case/report.json", "/tmp/{literal}]/report.json"] {
            assert_eq!(for_output(Path::new(value)).unwrap(), value);
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_glob_escapes_are_rejected() {
        for_output(Path::new(r"/tmp/literal\name/report.json")).unwrap_err();
    }
}
