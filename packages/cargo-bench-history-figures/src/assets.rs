//! The registry of everything the appendix embeds, and the read/write side of the
//! generated-asset contract.
//!
//! The appendix quotes concrete numbers on almost every page, and none of them is typed
//! by hand: each is rendered here and included by the book verbatim. That only holds if
//! the checked-in copies are kept in step with the code, which is what [`check`]
//! enforces — it re-renders everything in memory and reports whatever no longer matches,
//! so a behaviour change that the appendix describes fails a test instead of quietly
//! making the prose wrong.
//!
//! An asset is content plus the path it belongs at. Nothing else in the crate knows
//! where the book lives.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Where the generated assets live, relative to the workspace root.
///
/// The appendix includes them from here, so this path is the contract between this crate and
/// the book's Markdown. It lives in the library rather than in the binary so the freshness
/// test can reach it: a check only the binary can perform is a check that runs when someone
/// remembers the recipe, which is not often enough for content the book embeds verbatim.
pub const GENERATED_ROOT: &str = "packages/cargo-bench-history/book/src/appendix/generated";

/// The workspace root, located from this crate's manifest directory.
///
/// Tests run with an unspecified working directory, so the path to the book cannot be relative
/// to it. The manifest directory is fixed at compile time and the crate's position under
/// `packages/` is a property of the workspace layout, so two levels up is the root.
#[must_use]
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap_or(Path::new("."))
        .to_path_buf()
}

/// One generated file the book includes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Asset {
    /// Path relative to the appendix's generated-asset directory, using forward slashes
    /// so the registry reads the same on every platform.
    pub path: String,

    /// The file's full contents.
    pub content: String,
}

impl Asset {
    /// An asset at `path` holding `content`.
    #[must_use]
    pub fn new(path: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            content: content.into(),
        }
    }

    /// Where the asset belongs under `root`.
    #[must_use]
    pub fn location(&self, root: &Path) -> PathBuf {
        self.path
            .split('/')
            .fold(root.to_path_buf(), |path, segment| path.join(segment))
    }
}

/// Every asset the appendix embeds.
///
/// This is the crate's single source of truth: the writer, the freshness check and the
/// preview page all render from it, so none of them can drift from the others.
#[must_use]
pub fn assets() -> Vec<Asset> {
    let mut assets = Vec::new();
    assets.extend(crate::figures::assets());
    assets.sort_by(|left, right| left.path.cmp(&right.path));
    assets
}

/// Writes every asset under `root`, creating directories as needed.
///
/// # Errors
///
/// Propagates any filesystem failure.
pub fn write(root: &Path) -> io::Result<usize> {
    let assets = assets();
    for asset in &assets {
        let location = asset.location(root);
        if let Some(parent) = location.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&location, &asset.content)?;
    }
    Ok(assets.len())
}

/// Compares every asset against the copy checked in under `root`.
///
/// Returns a human-readable report of what differs, or `None` when everything matches.
/// Reporting all mismatches rather than the first keeps a regeneration from turning into
/// a sequence of one-at-a-time discoveries.
///
/// # Errors
///
/// Propagates any filesystem failure other than a missing file, which is reported as a
/// mismatch instead.
pub fn check(root: &Path) -> io::Result<Option<String>> {
    let mut problems = Vec::new();

    for asset in assets() {
        let location = asset.location(root);
        match fs::read_to_string(&location) {
            Ok(existing) if existing == asset.content => {}
            Ok(_) => problems.push(format!("  {} differs from the generated content", asset.path)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                problems.push(format!("  {} has not been generated yet", asset.path));
            }
            Err(error) => return Err(error),
        }
    }

    if problems.is_empty() {
        return Ok(None);
    }

    let mut report = String::from(
        "The generated appendix assets are out of date. Run `just book-figures` to \
         regenerate them, then review the diff — a change here means the pipeline's \
         behaviour no longer matches what the appendix describes.\n",
    );
    for problem in problems {
        writeln!(report, "{problem}").expect("writing to a String never fails");
    }
    Ok(Some(report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_paths_are_unique() {
        let assets = assets();
        let mut paths: Vec<&str> = assets.iter().map(|asset| asset.path.as_str()).collect();
        paths.sort_unstable();
        let count = paths.len();
        paths.dedup();

        assert_eq!(count, paths.len(), "two assets claim the same path");
    }

    #[test]
    fn assets_are_reproducible() {
        assert_eq!(assets(), assets());
    }

    #[test]
    fn a_relative_path_resolves_under_the_root() {
        let asset = Asset::new("figures/example.svg", "content");

        let location = asset.location(Path::new("root"));

        assert!(location.ends_with("figures/example.svg") || location.ends_with("figures\\example.svg"));
    }

    /// The appendix embeds these files verbatim, so a stale one publishes a page that
    /// contradicts the tool. Running the check as an ordinary test means a behaviour change
    /// that moves a figure fails the suite, rather than waiting for someone to remember
    /// `just book-figures-check`.
    #[test]
    fn the_checked_in_appendix_assets_are_current() {
        let root = workspace_root().join(GENERATED_ROOT);

        let report = check(&root).expect("reading the checked-in assets must not fail");

        assert!(report.is_none(), "{}", report.unwrap_or_default());
    }
}
