//! Path helpers shared by the command run edge.

use std::path::{Path, PathBuf};

/// Joins a relative `path` onto `base`, leaving an absolute `path` unchanged.
///
/// In production `base` is the process working directory, so a relative path
/// resolves exactly as the filesystem would have. Threading the base explicitly
/// (rather than relying on the process current directory) lets tests point each
/// command at its own workspace without a global `chdir`, so the suite need not be
/// forced serial.
#[must_use]
pub fn rebase(base: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn rebase_joins_relative_paths_and_keeps_absolute_ones() {
        let base = Path::new("/work/folo");
        assert_eq!(
            rebase(base, PathBuf::from("store")),
            PathBuf::from("/work/folo/store")
        );
        let absolute = if cfg!(windows) {
            PathBuf::from(r"C:\elsewhere\store")
        } else {
            PathBuf::from("/elsewhere/store")
        };
        assert_eq!(rebase(base, absolute.clone()), absolute);
    }
}
