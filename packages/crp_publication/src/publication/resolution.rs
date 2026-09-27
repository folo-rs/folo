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
#[cfg_attr(test, mutants::skip)] // Workspace/Git/file acquisition is integration-tested; archive and comparison decisions are unit-tested.
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
    let expected = source
        .closure(name, version, installation)?
        .ok_or_else(|| InstallationClosureMissing::new(name.to_owned(), "source"))?;
    // Normalize only a workspace identity this binary actually uses. A same-named registry
    // dependency remains a registry dependency, including its transitive declarations.
    for entry in &mut packaged.entries {
        let version = entry.version.to_string();
        let registry_identity = format!("{version} ({registry_source})");
        if entry.source.as_deref() == Some(registry_source)
            && published.get(&entry.name) == Some(&entry.version)
            && expected.get(&entry.name).is_some_and(|identities| {
                identities.contains(&version) && !identities.contains(&registry_identity)
            })
        {
            entry.source = None;
        }
    }
    let actual = packaged
        .closure(name, version, installation)?
        .ok_or_else(|| InstallationClosureMissing::new(name.to_owned(), "packaged"))?;
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
#[display("binary package archive {package} has no Cargo.lock")]
struct PackageLockfileMissing {
    package: String,
}

/// Identifies which lockfile cannot establish the requested binary's dependency closure.
#[ohno::error]
#[display("{location} Cargo.lock does not identify the installation closure of binary {package}")]
struct InstallationClosureMissing {
    package: String,
    location: &'static str,
}

#[ohno::error]
#[display("packaged dependency resolution introduces an unassessed identity for binary {package}")]
struct PackagedResolutionChanged {
    package: String,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use crp_workspace::manifest::{DependencySource, InstallationDependency};
    use flate2::{Compression, write::GzEncoder};
    use tar::{Builder, Header};

    use super::*;

    fn package_archive(path: &str, contents: &[u8]) -> Vec<u8> {
        let mut archive = Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        let mut header = Header::new_gnu();
        header.set_size(contents.len().try_into().unwrap());
        header.set_mode(0o644);
        header.set_cksum();
        archive.append_data(&mut header, path, contents).unwrap();
        archive.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn packaged_lockfile_requires_the_exact_archive_member_and_readable_contents() {
        let contents = b"version = 4\n";
        let archive = package_archive("tool-1.0.0/Cargo.lock", contents);
        assert_eq!(
            packaged_lockfile(&archive, "tool", "1.0.0").unwrap(),
            "version = 4\n"
        );
        assert!(
            packaged_lockfile(&archive, "other", "1.0.0")
                .unwrap_err()
                .find_source::<PackageLockfileMissing>()
                .is_some()
        );
        assert!(
            packaged_lockfile(&archive, "tool", "2.0.0")
                .unwrap_err()
                .find_source::<PackageLockfileMissing>()
                .is_some()
        );
        let invalid = package_archive("tool-1.0.0/Cargo.lock", &[0xff]);
        assert!(
            packaged_lockfile(&invalid, "tool", "1.0.0")
                .unwrap_err()
                .find_source::<PackageArchiveError>()
                .is_some()
        );
        assert!(
            packaged_lockfile(b"not an archive", "tool", "1.0.0")
                .unwrap_err()
                .find_source::<PackageArchiveError>()
                .is_some()
        );
    }

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

    #[test]
    fn registry_dependency_does_not_borrow_a_same_named_workspace_identity() {
        let registry = "registry+https://github.com/rust-lang/crates.io-index";
        let source = Lockfile::parse(
            &format!(
                "[[package]]\nname='binary'\nversion='1.0.0'\n\
                 dependencies=['dependency 1.0.0 ({registry})']\n\
                 [[package]]\nname='dependency'\nversion='1.0.0'\n\
                 [[package]]\nname='dependency'\nversion='1.0.0'\nsource='{registry}'\n\
                 dependencies=['leaf']\n\
                 [[package]]\nname='leaf'\nversion='1.0.0'\nsource='{registry}'\n"
            ),
            "source",
        )
        .unwrap();
        let published = BTreeMap::from([("dependency".to_owned(), Version::new(1, 0, 0))]);
        let mut installation = InstallationGraph::default();
        installation.insert(
            "binary".to_owned(),
            Version::new(1, 0, 0),
            vec![InstallationDependency {
                name: "dependency".to_owned(),
                requirement: Some("=1.0.0".parse().unwrap()),
                source: DependencySource::Registry(
                    "https://github.com/rust-lang/crates.io-index".to_owned(),
                ),
            }],
        );
        for leaf in ["1.0.0", "2.0.0"] {
            let packaged = Lockfile::parse(
                &format!(
                    "[[package]]\nname='binary'\nversion='1.0.0'\ndependencies=['dependency']\n\
                     [[package]]\nname='dependency'\nversion='1.0.0'\nsource='{registry}'\n\
                     dependencies=['leaf']\n\
                     [[package]]\nname='leaf'\nversion='{leaf}'\nsource='{registry}'\n"
                ),
                "packaged",
            )
            .unwrap();
            assert_eq!(
                compare(
                    &source,
                    packaged,
                    &installation,
                    &published,
                    "binary",
                    "1.0.0",
                    registry
                )
                .is_ok(),
                leaf == "1.0.0"
            );
        }
    }
}
