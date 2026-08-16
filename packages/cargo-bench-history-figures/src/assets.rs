//! The registry of everything the appendix embeds, and the read/write side of the
//! generated-asset contract.
//!
//! The appendix quotes concrete numbers on almost every page, and none of them is typed
//! by hand: each is rendered here and included by the book verbatim. That only holds if
//! the checked-in copies are kept in step with the code, which is what [`check`]
//! enforces — it re-renders everything in memory and reports whatever no longer matches
//! the registry, including leftover files from a rename or removal, so a behaviour
//! change that the appendix describes fails a test instead of quietly making the prose
//! wrong.
//!
//! An asset is content plus the path it belongs at. Nothing else in the crate knows
//! where the book lives.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};
use std::{fs, io};

/// Where the generated assets live, relative to the workspace root.
///
/// The appendix includes them from here, so this path is the contract between this crate and
/// the book's Markdown. It lives in the library rather than in the binary so the freshness
/// test can reach it: a check only the binary can perform is a check that runs when someone
/// remembers the recipe, which is not often enough for content the book embeds verbatim.
pub const GENERATED_ROOT: &str = "packages/cargo-bench-history/book/src/appendix/generated";

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

/// Writes the registry under `root` and deletes any other regular file there.
///
/// Registered paths must stay under `root`.
///
/// # Errors
///
/// Propagates any filesystem failure.
pub fn write(root: &Path) -> io::Result<usize> {
    write_registry(root, &assets())
}

/// Reports registered files that differ and extra files under `root`.
///
/// Returns a human-readable report of what differs, or `None` when everything matches.
/// Reporting all mismatches rather than the first keeps a regeneration from turning into
/// a sequence of one-at-a-time discoveries. Registered paths must stay under `root`.
///
/// # Errors
///
/// Propagates any filesystem failure other than a missing registered file, which is
/// reported as a mismatch instead.
pub fn check(root: &Path) -> io::Result<Option<String>> {
    check_registry(root, &assets())
}

/// Writes `assets` under `root` after deleting regular files that are not in that set.
fn write_registry(root: &Path, assets: &[Asset]) -> io::Result<usize> {
    let registered = registered_paths(assets)?;
    for (_relative, extra) in extra_files(root, &registered)? {
        fs::remove_file(&extra).map_err(|error| wrap_io(&error, "delete", &extra))?;
    }
    for asset in assets {
        let location = asset.location(root);
        if let Some(parent) = location.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| wrap_io(&error, "create directory", parent))?;
        }
        fs::write(&location, &asset.content)
            .map_err(|error| wrap_io(&error, "write", &location))?;
    }
    Ok(assets.len())
}

/// Compares `assets` to the files under `root`, including leftovers not in the set.
fn check_registry(root: &Path, assets: &[Asset]) -> io::Result<Option<String>> {
    let registered = registered_paths(assets)?;
    let mut problems = Vec::new();

    for asset in assets {
        let location = asset.location(root);
        match fs::read_to_string(&location) {
            Ok(existing) if existing == asset.content => {}
            Ok(_) => problems.push(format!(
                "  {} differs from the generated content",
                asset.path
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                problems.push(format!("  {} has not been generated yet", asset.path));
            }
            Err(error) => return Err(wrap_io(&error, "read", &location)),
        }
    }

    for (relative, _path) in extra_files(root, &registered)? {
        problems.push(format!(
            "  {relative} is not in the generated-asset registry"
        ));
    }

    if problems.is_empty() {
        return Ok(None);
    }

    let mut report = String::from(
        "The generated appendix assets are out of date: a checked-in copy differs from what \
         the generator now produces. Run `just book-figures` to regenerate them, then review \
         the diff — for a behaviour-derived asset a change means the pipeline changed, while a \
         presentation-only asset may simply have been restyled.\n",
    );
    for problem in problems {
        writeln!(report, "{problem}").expect("writing to a String never fails");
    }
    Ok(Some(report))
}

/// Relative registry paths, after rejecting any that would escape the generated root.
fn registered_paths(assets: &[Asset]) -> io::Result<HashSet<String>> {
    let mut paths = HashSet::new();
    for asset in assets {
        validate_registered_path(&asset.path)?;
        paths.insert(asset.path.clone());
    }
    Ok(paths)
}

/// Rejects empty, `.`, `..`, and platform-absolute segments so a registry entry
/// cannot write or compare outside the generated root.
fn validate_registered_path(path: &str) -> io::Result<()> {
    if path.split('/').any(|segment| !is_plain_segment(segment)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("asset path {path} is not under the generated root"),
        ));
    }
    Ok(())
}

fn is_plain_segment(segment: &str) -> bool {
    let mut components = Path::new(segment).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

/// Regular files under `root` whose relative path is not in `registered`.
fn extra_files(root: &Path, registered: &HashSet<String>) -> io::Result<Vec<(String, PathBuf)>> {
    let mut extras = Vec::new();
    collect_extras(root, root, registered, &mut extras)?;
    extras.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(extras)
}

/// Recursively collects unregistered regular files, starting at `dir`.
fn collect_extras(
    root: &Path,
    dir: &Path,
    registered: &HashSet<String>,
    extras: &mut Vec<(String, PathBuf)>,
) -> io::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound && dir == root => {
            return Ok(());
        }
        Err(error) => return Err(wrap_io(&error, "read directory", dir)),
    };

    for entry in entries {
        let entry = entry.map_err(|error| wrap_io(&error, "read directory", dir))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| wrap_io(&error, "inspect", &path))?;
        if file_type.is_dir() {
            debug_assert!(
                path.starts_with(dir) && path != dir,
                "directory walk must descend into a child entry.",
            );
            collect_extras(root, &path, registered, extras)?;
        } else if file_type.is_file() {
            let relative =
                relative_posix(root, &path).unwrap_or_else(|| path.display().to_string());
            if !registered.contains(&relative) {
                extras.push((relative, path));
            }
        }
    }
    Ok(())
}

/// The `/`-separated path of `path` relative to `root`, if every component is ordinary.
fn relative_posix(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let mut posix = String::new();
    for component in relative.components() {
        let Component::Normal(segment) = component else {
            return None;
        };
        let segment = segment.to_str()?;
        if !posix.is_empty() {
            posix.push('/');
        }
        posix.push_str(segment);
    }
    Some(posix)
}

/// Attaches the attempted operation and path to a filesystem failure.
fn wrap_io(error: &io::Error, operation: &str, path: &Path) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("failed to {operation} {}: {error}", path.display()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "plotters SVG generation is host graphics, not memory-safety-relevant, and exceeds the Miri CI budget"
    )]
    fn asset_paths_are_unique() {
        let assets = assets();
        let mut paths: Vec<&str> = assets.iter().map(|asset| asset.path.as_str()).collect();
        paths.sort_unstable();
        let count = paths.len();
        paths.dedup();

        assert_eq!(count, paths.len(), "two assets claim the same path");
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "plotters SVG generation is host graphics, not memory-safety-relevant, and exceeds the Miri CI budget"
    )]
    fn assets_are_reproducible() {
        assert_eq!(assets(), assets());
    }

    #[test]
    fn a_relative_path_resolves_under_the_root() {
        let asset = Asset::new("figures/example.svg", "content");

        let location = asset.location(Path::new("root"));

        assert!(
            location.ends_with("figures/example.svg") || location.ends_with("figures\\example.svg")
        );
    }

    #[test]
    fn a_parent_segment_is_rejected() {
        let asset = Asset::new("../escape.txt", "no");

        let check = check_registry(Path::new("root"), std::slice::from_ref(&asset));
        let write = write_registry(Path::new("root"), &[asset]);

        assert_eq!(check.unwrap_err().kind(), io::ErrorKind::InvalidInput);
        assert_eq!(write.unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    #[cfg_attr(miri, ignore = "uses the real filesystem, which Miri does not model")]
    fn check_reports_an_unregistered_file() {
        let root = tempfile::tempdir().unwrap();
        let asset = Asset::new("kept.txt", "kept\n");
        write_registry(root.path(), std::slice::from_ref(&asset)).unwrap();
        fs::create_dir_all(root.path().join("nested")).unwrap();
        fs::write(root.path().join("nested/stale.txt"), "leftover").unwrap();

        let report = check_registry(root.path(), &[asset]).unwrap().unwrap();

        assert!(report.contains("nested/stale.txt"));
    }

    #[test]
    #[cfg_attr(miri, ignore = "uses the real filesystem, which Miri does not model")]
    fn write_deletes_an_unregistered_file() {
        let root = tempfile::tempdir().unwrap();
        let asset = Asset::new("kept.txt", "kept\n");
        write_registry(root.path(), std::slice::from_ref(&asset)).unwrap();
        fs::create_dir_all(root.path().join("nested")).unwrap();
        fs::write(root.path().join("nested/stale.txt"), "leftover").unwrap();

        write_registry(root.path(), std::slice::from_ref(&asset)).unwrap();

        assert!(!root.path().join("nested/stale.txt").exists());
        assert_eq!(
            fs::read_to_string(root.path().join("kept.txt")).unwrap(),
            "kept\n"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "uses the real filesystem, which Miri does not model")]
    fn a_renamed_asset_leaves_the_old_path_as_an_extra() {
        let root = tempfile::tempdir().unwrap();
        write_registry(root.path(), &[Asset::new("old.txt", "same\n")]).unwrap();
        let renamed = Asset::new("new.txt", "same\n");

        let report = check_registry(root.path(), std::slice::from_ref(&renamed))
            .unwrap()
            .unwrap();
        assert!(report.contains("old.txt"));
        assert!(report.contains("new.txt"));

        write_registry(root.path(), &[renamed]).unwrap();

        assert!(!root.path().join("old.txt").exists());
        assert_eq!(
            fs::read_to_string(root.path().join("new.txt")).unwrap(),
            "same\n"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "uses the real filesystem, which Miri does not model")]
    fn check_reports_a_stale_registered_file() {
        let root = tempfile::tempdir().unwrap();
        let asset = Asset::new("kept.txt", "fresh\n");
        write_registry(root.path(), std::slice::from_ref(&asset)).unwrap();
        fs::write(root.path().join("kept.txt"), "stale\n").unwrap();

        let report = check_registry(root.path(), &[asset]).unwrap().unwrap();

        assert!(report.contains("kept.txt"));
    }

    #[test]
    #[cfg_attr(miri, ignore = "uses the real filesystem, which Miri does not model")]
    fn check_accepts_a_matching_registry() {
        let root = tempfile::tempdir().unwrap();
        let asset = Asset::new("kept.txt", "fresh\n");
        write_registry(root.path(), std::slice::from_ref(&asset)).unwrap();

        assert!(check_registry(root.path(), &[asset]).unwrap().is_none());
    }

    /// The appendix embeds these files verbatim, so a stale one publishes a page that
    /// contradicts the tool. Running the check as an ordinary test means a behaviour change
    /// that moves a figure fails the suite, rather than waiting for someone to remember
    /// `just book-figures-check`.
    #[test]
    #[cfg_attr(
        miri,
        ignore = "plotters SVG generation is host graphics, not memory-safety-relevant, and exceeds the Miri CI budget"
    )]
    fn the_checked_in_appendix_assets_are_current() {
        let root = workspace_root().join(GENERATED_ROOT);

        let report = check(&root).expect("reading the checked-in assets must not fail");

        assert!(report.is_none(), "{}", report.unwrap_or_default());
    }
}
