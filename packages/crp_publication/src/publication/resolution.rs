//! Confirms Cargo's packaged binary resolution matches the reviewed source resolution.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;

use crp_workspace::lockfile::{InstallationGraph, Lockfile};
use crp_workspace::metadata::load_tracked_work_tree;
use flate2::read::GzDecoder;
use ohno::AppError;
use semver::Version;
use tar::Archive;

use crate::ReadFileError;

/// Checks archive bytes without extracting files or changing the source workspace.
pub fn verify_packaged_closure(
    manifest: &Path,
    archive: &[u8],
    name: &str,
    version: &str,
    registry_source: &str,
) -> Result<(), AppError> {
    let (workspace, _) = load_tracked_work_tree(manifest)?;
    let source = workspace.workspace_root.join("Cargo.lock");
    let source_lock =
        fs::read_to_string(&source).map_err(|error| ReadFileError::caused_by(&source, error))?;
    let source_lock = Lockfile::parse(&source_lock, "publication source Cargo.lock")?;
    let archive_lock = packaged_lockfile(archive, name, version)?;
    let archive_lock = Lockfile::parse(&archive_lock, "packaged Cargo.lock")?;
    let published = workspace
        .packages
        .iter()
        .map(|package| {
            (
                package.manifest.name.clone(),
                package.manifest.version.clone(),
            )
        })
        .collect();
    compare(
        &source_lock,
        archive_lock,
        &workspace.installation,
        &published,
        name,
        version,
        registry_source,
    )
}

fn packaged_lockfile(bytes: &[u8], name: &str, version: &str) -> Result<String, AppError> {
    let mut archive = Archive::new(GzDecoder::new(bytes));
    let expected = format!("{name}-{version}/Cargo.lock");
    for entry in archive.entries().map_err(PackageArchiveError::caused_by)? {
        let mut entry = entry.map_err(PackageArchiveError::caused_by)?;
        if entry.path().map_err(PackageArchiveError::caused_by)? == Path::new(&expected) {
            let mut contents = String::new();
            entry
                .read_to_string(&mut contents)
                .map_err(PackageArchiveError::caused_by)?;
            return Ok(contents);
        }
    }
    Err(PackageLockfileMissing::new(name.to_owned()).into())
}

fn compare(
    source: &Lockfile,
    mut packaged: Lockfile,
    installation: &InstallationGraph,
    published: &BTreeMap<String, Version>,
    name: &str,
    version: &str,
    registry_source: &str,
) -> Result<(), AppError> {
    // Cargo replaces publishable workspace paths with registry identities. Normalize only
    // that known transformation; external patches and other registry/Git sources remain visible.
    for entry in &mut packaged.entries {
        if entry.source.as_deref() == Some(registry_source)
            && published.get(&entry.name) == Some(&entry.version)
            && source.entries.iter().any(|original| {
                original.source.is_none()
                    && original.name == entry.name
                    && original.version == entry.version
            })
        {
            entry.source = None;
        }
    }
    let expected = source
        .closure(name, version, installation)?
        .ok_or_else(|| PackageLockfileMissing::new(name.to_owned()))?;
    let actual = packaged
        .closure(name, version, installation)?
        .ok_or_else(|| PackageLockfileMissing::new(name.to_owned()))?;
    // Workspace members contribute optional features to Cargo.lock even when this binary
    // does not enable them. Packaging can prune those branches, but cannot select any
    // dependency identity outside the assessed installation closure.
    if actual.iter().any(|(dependency, identities)| {
        expected
            .get(dependency)
            .is_none_or(|expected| !identities.is_subset(expected))
    }) {
        return Err(PackagedResolutionChanged::new(name.to_owned()).into());
    }
    Ok(())
}

#[ohno::error]
#[display("cannot inspect the Cargo package archive")]
struct PackageArchiveError;

#[ohno::error]
#[display("binary package {package} has no complete installation lockfile")]
struct PackageLockfileMissing {
    package: String,
}

#[ohno::error]
#[display("packaged dependency resolution introduces an unassessed identity for binary {package}")]
struct PackagedResolutionChanged {
    package: String,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn lockfile(dependency: &str, source: &str) -> Lockfile {
        let source = if source.is_empty() {
            String::new()
        } else {
            format!("source = {source:?}\n")
        };
        Lockfile::parse(
            &format!(
                "[[package]]\nname='binary'\nversion='1.0.0'\ndependencies=['dependency']\n\
                 [[package]]\nname='dependency'\nversion='{dependency}'\n{source}"
            ),
            "fixture",
        )
        .unwrap()
    }

    #[test]
    fn normalizes_only_published_workspace_identities() {
        let registry = "registry+https://github.com/rust-lang/crates.io-index";
        let source = lockfile("1.0.0", "");
        let published = BTreeMap::from([("dependency".to_owned(), Version::new(1, 0, 0))]);
        compare(
            &source,
            lockfile("1.0.0", registry),
            &InstallationGraph::default(),
            &published,
            "binary",
            "1.0.0",
            registry,
        )
        .unwrap();
        for (version, location) in [
            ("1.0.1", registry),
            ("1.0.0", "registry+https://another.invalid/index"),
        ] {
            compare(
                &source,
                lockfile(version, location),
                &InstallationGraph::default(),
                &published,
                "binary",
                "1.0.0",
                registry,
            )
            .unwrap_err();
        }
        compare(
            &source,
            lockfile("1.0.0", registry),
            &InstallationGraph::default(),
            &BTreeMap::new(),
            "binary",
            "1.0.0",
            registry,
        )
        .unwrap_err();
    }
}
