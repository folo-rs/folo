//! Rendering of stored paths for people rather than for comparison.

use std::path::Path;

/// Renders a stored path the way the user typed it rather than the way it is
/// compared.
///
/// Launch directories are stored canonicalized, which on Windows means the
/// extended-length form (`\\?\C:\work`). That prefix exists to lift path-length
/// and parsing limits, carries no meaning for a reader, and is not what anyone
/// typed. Auto-detect still compares the stored canonical paths; only the
/// rendering drops the prefix. Ref: docs/design.md, "Listing sessions".
pub(crate) fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{share}");
    }
    text.strip_prefix(r"\\?\").unwrap_or(&text).to_string()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn strips_the_extended_length_prefix() {
        assert_eq!(display_path(&PathBuf::from(r"\\?\C:\Source")), r"C:\Source");
    }

    #[test]
    fn restores_the_network_share_form() {
        assert_eq!(
            display_path(&PathBuf::from(r"\\?\UNC\server\share\work")),
            r"\\server\share\work"
        );
    }

    #[test]
    fn leaves_an_ordinary_path_alone() {
        assert_eq!(display_path(&PathBuf::from(r"C:\Source")), r"C:\Source");
        assert_eq!(display_path(&PathBuf::from("/work")), "/work");
    }
}
