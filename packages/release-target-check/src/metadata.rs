use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ohno::AppError;
use semver::Version;
use serde::Deserialize;

use crate::Repository;
use crate::repository::{VerificationError, canonicalize};

/// Validates Cargo's identity response before the release checker assesses package content.
#[derive(Debug, Deserialize)]
pub struct Metadata {
    packages: Vec<Package>,
    workspace_members: Vec<String>,
    workspace_root: PathBuf,
}

impl Metadata {
    pub fn parse(bytes: &[u8]) -> Result<Self, AppError> {
        serde_json::from_slice(bytes).map_err(|error| {
            VerificationError::caused_by("cannot decode candidate Cargo metadata", error).into()
        })
    }

    // Wire real-system adapters here; the same input selection and validation sequence below
    // runs against in-memory callbacks in unit tests.
    #[cfg_attr(test, mutants::skip)]
    pub fn validate_inputs(
        &self,
        repository: &Repository,
        manifest: &Path,
    ) -> Result<(), AppError> {
        self.validate_inputs_using(
            manifest,
            |path| repository.require_tracked(path).map(|_| ()),
            lockfile_exists,
            canonicalize,
        )
    }

    fn validate_inputs_using(
        &self,
        manifest: &Path,
        mut require_tracked: impl FnMut(&Path) -> Result<(), AppError>,
        lockfile_exists: impl FnOnce(&Path) -> Result<bool, AppError>,
        canonicalize: impl FnOnce(&Path) -> Result<PathBuf, AppError>,
    ) -> Result<(), AppError> {
        require_tracked(&self.workspace_root.join("Cargo.toml"))?;
        let lockfile = self.workspace_root.join("Cargo.lock");
        if lockfile_exists(&lockfile)? {
            require_tracked(&lockfile)?;
        }
        for package in &self.packages {
            require_tracked(&package.manifest_path)?;
        }
        let workspace_root = canonicalize(&self.workspace_root)?;
        validate_manifest_location(manifest, &workspace_root)
    }

    pub(crate) fn validate_packages(
        &self,
        required: &BTreeMap<String, Version>,
        verbose: bool,
    ) -> Result<(), AppError> {
        for (name, version) in required {
            let mut matches = self.packages.iter().filter(|package| package.name == *name);
            let package = matches.next().ok_or_else(|| {
                VerificationError::new(format!(
                    "requested package is absent from candidate metadata: {name}"
                ))
            })?;
            if matches.next().is_some()
                || !self.workspace_members.contains(&package.id)
                || package.publish.as_ref().is_some_and(Vec::is_empty)
                || package.version != version.to_string()
            {
                return Err(VerificationError::new(format!(
                    "candidate must contain one publishable workspace member {name}@{version}; \
                     metadata reports version {}",
                    package.version
                ))
                .into());
            }
            if verbose {
                eprintln!(
                    "[release-target-check] {name}@{version} matches a tracked publishable workspace \
                     member; locked, offline, no-deps metadata supplies identity without refreshing \
                     dependency resolution"
                );
            }
        }
        Ok(())
    }
}

// Filesystem lookup failures are exercised with the integration fixture's symlink loop.
#[cfg_attr(test, mutants::skip)]
fn lockfile_exists(path: &Path) -> Result<bool, AppError> {
    path.try_exists().map_err(|error| {
        VerificationError::caused_by("cannot inspect candidate workspace lockfile", error).into()
    })
}

fn validate_manifest_location(manifest: &Path, workspace_root: &Path) -> Result<(), AppError> {
    if !manifest.starts_with(workspace_root) {
        return Err(VerificationError::new(
            "candidate manifest is outside the metadata workspace root",
        )
        .into());
    }
    Ok(())
}

/// Carries the declared identity and publication eligibility of a metadata package.
#[derive(Debug, Deserialize)]
struct Package {
    name: String,
    version: String,
    id: String,
    manifest_path: PathBuf,
    /// Cargo uses null for unrestricted publication and an empty array for disabled publication.
    publish: Option<Vec<String>>,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::RefCell;
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use serde_json::json;
    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(Metadata: RefUnwindSafe, UnwindSafe);

    #[test]
    fn requires_manifest_within_metadata_workspace() {
        let root = Path::new("workspace");
        for manifest in [
            root.join("Cargo.toml"),
            root.join("member").join("Cargo.toml"),
        ] {
            validate_manifest_location(&manifest, root).unwrap();
        }
        for manifest in [
            PathBuf::from("Cargo.toml"),
            Path::new("workspace-other").join("Cargo.toml"),
        ] {
            let error = validate_manifest_location(&manifest, root).unwrap_err();
            assert!(error.find_source::<VerificationError>().is_some());
        }
    }

    #[test]
    fn selects_every_manifest_and_only_an_existing_lockfile() {
        let root = Path::new("workspace");
        let canonical_root = Path::new("canonical-workspace");
        let mut metadata = sample(root);
        metadata.packages = vec![
            package(&root.join("first"), "first"),
            package(&root.join("second"), "second"),
        ];

        for exists in [false, true] {
            let calls = RefCell::new(Vec::new());
            metadata
                .validate_inputs_using(
                    &canonical_root.join("Cargo.toml"),
                    |path| {
                        calls.borrow_mut().push(("tracked", path.to_owned()));
                        Ok(())
                    },
                    |path| {
                        calls.borrow_mut().push(("exists", path.to_owned()));
                        Ok(exists)
                    },
                    |path| {
                        calls.borrow_mut().push(("canonicalize", path.to_owned()));
                        Ok(canonical_root.to_owned())
                    },
                )
                .unwrap();

            let mut expected = vec![
                ("tracked", root.join("Cargo.toml")),
                ("exists", root.join("Cargo.lock")),
            ];
            if exists {
                expected.push(("tracked", root.join("Cargo.lock")));
            }
            expected.extend([
                ("tracked", root.join("first").join("Cargo.toml")),
                ("tracked", root.join("second").join("Cargo.toml")),
                ("canonicalize", root.to_owned()),
            ]);
            assert_eq!(*calls.borrow(), expected);
        }
    }

    #[test]
    fn input_validation_propagates_each_adapter_failure() {
        let root = Path::new("workspace");
        let mut metadata = sample(root);
        metadata.packages = vec![package(&root.join("member"), "widget")];
        let expected = [
            ("tracked", root.join("Cargo.toml")),
            ("exists", root.join("Cargo.lock")),
            ("tracked", root.join("Cargo.lock")),
            ("tracked", root.join("member").join("Cargo.toml")),
            ("canonicalize", root.to_owned()),
        ];

        for failed_call in 0..expected.len() {
            let calls = RefCell::new(Vec::new());
            let record = |operation, path: &Path| -> Result<(), AppError> {
                calls.borrow_mut().push((operation, path.to_owned()));
                if calls.borrow().len() == failed_call + 1 {
                    Err(VerificationError::new("adapter failed").into())
                } else {
                    Ok(())
                }
            };
            let error = metadata
                .validate_inputs_using(
                    &root.join("Cargo.toml"),
                    |path| record("tracked", path),
                    |path| record("exists", path).map(|()| true),
                    |path| record("canonicalize", path).map(|()| path.to_owned()),
                )
                .unwrap_err();
            assert!(error.find_source::<VerificationError>().is_some());
            assert_eq!(*calls.borrow(), expected.get(..=failed_call).unwrap());
        }
    }

    #[test]
    fn input_validation_uses_the_canonical_root_for_containment() {
        let root = Path::new("workspace");
        let error = sample(root)
            .validate_inputs_using(
                &root.join("Cargo.toml"),
                |_| Ok(()),
                |_| Ok(false),
                |_| Ok(PathBuf::from("different-workspace")),
            )
            .unwrap_err();
        assert!(error.find_source::<VerificationError>().is_some());
    }

    fn sample(root: &Path) -> Metadata {
        Metadata {
            workspace_root: root.to_owned(),
            workspace_members: vec!["widget-id".into()],
            packages: vec![package(root, "widget")],
        }
    }

    fn package(root: &Path, name: &str) -> Package {
        Package {
            name: name.into(),
            id: format!("{name}-id"),
            version: "1.0.0".into(),
            manifest_path: root.join("Cargo.toml"),
            publish: None,
        }
    }

    fn required() -> BTreeMap<String, Version> {
        BTreeMap::from([("widget".into(), Version::new(1, 0, 0))])
    }

    #[test]
    fn decodes_required_identity_fields_and_tolerates_unrelated_fields() {
        let input = json!({
            "workspace_root": "workspace",
            "workspace_members": ["widget-id"],
            "packages": [{
                "name": "widget",
                "version": "1.0.0",
                "id": "widget-id",
                "manifest_path": "workspace/Cargo.toml",
                "publish": null,
                "unrelated": true
            }],
            "unrelated": true
        });
        let input = serde_json::to_vec(&input).unwrap();
        let metadata = Metadata::parse(&input).unwrap();
        assert_eq!(metadata.workspace_root, PathBuf::from("workspace"));
        assert_eq!(
            metadata.packages.first().unwrap().manifest_path,
            PathBuf::from("workspace").join("Cargo.toml")
        );
        metadata.validate_packages(&required(), false).unwrap();
    }

    #[test]
    fn rejects_malformed_metadata_and_missing_required_fields() {
        for bytes in [
            b"not json".as_slice(),
            b"[]",
            b"{}",
            br#"{"workspace_root":null,"packages":[],"workspace_members":[]}"#,
            br#"{"workspace_root":".","packages":[{}],"workspace_members":[]}"#,
        ] {
            let error = Metadata::parse(bytes).unwrap_err();
            assert!(error.find_source::<VerificationError>().is_some());
            assert!(error.find_source::<serde_json::Error>().is_some());
        }
    }

    #[test]
    fn accepts_publishable_package_selections_independently_of_metadata_order() {
        for publication in [None, Some(vec!["crates-io".into()])] {
            let root = Path::new("workspace");
            let mut metadata = sample(root);
            metadata.packages.first_mut().unwrap().publish = publication;
            metadata.packages.insert(0, package(root, "another"));
            metadata.workspace_members.push("another-id".into());
            let mut required = required();
            required.insert("another".into(), Version::new(1, 0, 0));
            metadata.validate_packages(&required, false).unwrap();
            metadata.validate_packages(&required, true).unwrap();
        }
    }

    #[test]
    fn rejects_absent_duplicate_nonmember_private_and_mismatched_packages() {
        let root = Path::new("workspace");
        let required = required();

        let mut absent = sample(root);
        absent.packages.clear();
        let mut duplicate = sample(root);
        duplicate.packages.push(package(root, "widget"));
        let mut nonmember = sample(root);
        nonmember.workspace_members.clear();
        let mut private = sample(root);
        private.packages.first_mut().unwrap().publish = Some(Vec::new());
        let mut mismatch = sample(root);
        mismatch.packages.first_mut().unwrap().version = "1.1.0".into();
        let mut build_mismatch = sample(root);
        build_mismatch.packages.first_mut().unwrap().version = "1.0.0+other".into();

        for metadata in [
            absent,
            duplicate,
            nonmember,
            private,
            mismatch,
            build_mismatch,
        ] {
            let error = metadata.validate_packages(&required, false).unwrap_err();
            assert!(error.find_source::<VerificationError>().is_some());
        }
    }

    #[test]
    fn checks_every_requested_package() {
        let root = Path::new("workspace");
        let mut metadata = sample(root);
        metadata.packages.push(package(root, "other"));
        metadata.workspace_members.push("other-id".into());
        let mut required = required();
        required.insert("other".into(), Version::new(1, 0, 0));
        required.insert("widget".into(), Version::new(2, 0, 0));
        _ = metadata.validate_packages(&required, false).unwrap_err();
    }
}
