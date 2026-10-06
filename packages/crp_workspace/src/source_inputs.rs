//! Source locations included in captured evidence.

use std::borrow::Borrow;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use ohno::AppError;
use toml_edit::{DocumentMut, Item, TableLike, Value};

use crate::ReadFileError;
use crate::manifest::{for_each_dependency_table, parse_document};

/// Discovers reserved files and recursively acquired source directories.
///
/// Paths retain their acquired spelling. Callers add Git-tracked files and decide whether
/// dependency locations are relocatable.
#[derive(Debug, Default)]
pub struct SourceInputs {
    pub files: BTreeSet<PathBuf>,
    pub source_directories: BTreeSet<PathBuf>,
    /// Selected dependency spellings retained separately from relocatable capture identities.
    pub declared_paths: BTreeSet<PathBuf>,
}

impl SourceInputs {
    // Connects native manifest acquisition to the unit-tested workspace inventory.
    #[cfg_attr(test, mutants::skip)]
    pub fn discover(
        root: &Path,
        workspace_root: &Path,
        manifests: &[PathBuf],
        resolve_dependency: impl FnMut(&Path, &Path) -> Result<PathBuf, AppError>,
    ) -> Result<Self, AppError> {
        Self::discover_with_documents(
            root,
            workspace_root,
            manifests,
            resolve_dependency,
            read_document,
        )
    }

    /// Shares freshly acquired documents without retaining mutable workspace observations.
    // Native root alias resolution is shared with ordinary discovery.
    #[cfg_attr(test, mutants::skip)]
    pub fn discover_with_documents<D: Borrow<DocumentMut>>(
        root: &Path,
        workspace_root: &Path,
        manifests: &[PathBuf],
        resolve_dependency: impl FnMut(&Path, &Path) -> Result<PathBuf, AppError>,
        document: impl FnMut(&Path) -> Result<D, AppError>,
    ) -> Result<Self, AppError> {
        let inputs = Self::dependencies_with(
            manifests.iter().chain([&workspace_root.join("Cargo.toml")]),
            resolve_dependency,
            document,
        )?;
        // Cargo and Git may report different aliases of the same directory. Ancestor
        // membership needs resolved identities, not those independently acquired spellings.
        let resolved_root = fs::canonicalize(root)?;
        let resolved_workspace = fs::canonicalize(workspace_root)?;
        let workspace_root = root.join(
            resolved_workspace
                .strip_prefix(resolved_root)
                .map_err(|error| ReadFileError::caused_by(workspace_root, error))?,
        );
        Ok(inputs.workspace(root, &workspace_root, manifests))
    }

    fn workspace(mut self, root: &Path, workspace_root: &Path, manifests: &[PathBuf]) -> Self {
        self.files.insert(workspace_root.join("Cargo.lock"));
        for directory in workspace_root
            .ancestors()
            .take_while(|directory| directory.starts_with(root))
        {
            self.files.insert(directory.join(".cargo/config"));
            self.files.insert(directory.join(".cargo/config.toml"));
        }
        for manifest in manifests {
            self.files.insert(manifest.clone());
            let directory = manifest
                .parent()
                .expect("a manifest has a parent directory");
            self.files.insert(directory.join("build.rs"));
            self.source_directories.insert(directory.join("src"));
        }
        self
    }

    // Only this adapter reads manifests; graph selection is exercised with in-process inputs.
    #[cfg_attr(test, mutants::skip)]
    pub fn dependencies<'a>(
        manifests: impl IntoIterator<Item = &'a PathBuf>,
        resolve_dependency: impl FnMut(&Path, &Path) -> Result<PathBuf, AppError>,
    ) -> Result<Self, AppError> {
        Self::dependencies_with(manifests, resolve_dependency, read_document)
    }

    fn dependencies_with<'a, D: Borrow<DocumentMut>>(
        manifests: impl IntoIterator<Item = &'a PathBuf>,
        mut resolve_dependency: impl FnMut(&Path, &Path) -> Result<PathBuf, AppError>,
        mut document: impl FnMut(&Path) -> Result<D, AppError>,
    ) -> Result<Self, AppError> {
        let mut inputs = Self::default();
        let mut pending: BTreeSet<PathBuf> = manifests.into_iter().cloned().collect();
        let mut visited = BTreeSet::new();
        while let Some(manifest) = pending.pop_first() {
            if !visited.insert(manifest.clone()) {
                continue;
            }
            inputs.files.insert(manifest.clone());
            let document = document(&manifest)?;
            let document = document.borrow();
            let parent = manifest
                .parent()
                .expect("a manifest has a parent directory");
            inputs.explicit_sources(parent, document);
            let mut dependencies = Vec::new();
            for_each_dependency_table(document.as_table(), &mut |_, table| {
                dependency_paths(table, &mut dependencies);
            });
            if let Some(workspace) = document.get("workspace").and_then(Item::as_table_like) {
                for_each_dependency_table(workspace, &mut |_, table| {
                    dependency_paths(table, &mut dependencies);
                });
            }
            if let Some(patches) = document.get("patch").and_then(Item::as_table_like) {
                for (_, patch) in patches.iter() {
                    if let Some(table) = patch.as_table_like() {
                        dependency_paths(table, &mut dependencies);
                    }
                }
            }
            if let Some(replacements) = document.get("replace").and_then(Item::as_table_like) {
                dependency_paths(replacements, &mut dependencies);
            }
            for dependency in dependencies {
                // Canonical dependency identities bound traversal, but cannot retain the
                // selected link entries which artifact cleanup must also leave untouched.
                let declared = parent.join(&dependency);
                inputs
                    .declared_paths
                    .extend([declared.join("Cargo.toml"), declared.join("src")]);
                let directory = resolve_dependency(&manifest, Path::new(&dependency))?;
                inputs.source_directories.insert(directory.join("src"));
                pending.insert(directory.join("Cargo.toml"));
            }
        }
        Ok(inputs)
    }

    fn explicit_sources(&mut self, directory: &Path, document: &DocumentMut) {
        if let Some(package) = document.get("package").and_then(Item::as_table_like) {
            match package.get("build") {
                Some(build) if build.as_bool() == Some(false) => {}
                build => {
                    // Cargo discovers the conventional build script unless disabled or
                    // redirected explicitly. This also applies to nonmember dependencies.
                    let path = build.and_then(Item::as_str).unwrap_or("build.rs");
                    self.files.insert(directory.join(path));
                }
            }
        }
        if let Some(path) = document
            .get("lib")
            .and_then(Item::as_table_like)
            .and_then(|target| target.get("path"))
            .and_then(Item::as_str)
        {
            self.files.insert(directory.join(path));
        }
        for kind in ["bin", "example", "test", "bench"] {
            let Some(targets) = document.get(kind) else {
                continue;
            };
            if let Some(targets) = targets.as_array_of_tables() {
                self.files.extend(targets.iter().filter_map(|target| {
                    target
                        .get("path")
                        .and_then(Item::as_str)
                        .map(|path| directory.join(path))
                }));
            } else if let Some(targets) = targets.as_array() {
                self.files.extend(targets.iter().filter_map(|target| {
                    target
                        .as_inline_table()
                        .and_then(|target| target.get("path"))
                        .and_then(Value::as_str)
                        .map(|path| directory.join(path))
                }));
            }
        }
    }
}

// Both native entry points acquire complete current bytes before parsing.
#[cfg_attr(test, mutants::skip)]
fn read_document(path: &Path) -> Result<DocumentMut, AppError> {
    let text = fs::read_to_string(path).map_err(|error| ReadFileError::caused_by(path, error))?;
    parse_document(path, &text)
}

fn dependency_paths(table: &dyn TableLike, paths: &mut Vec<String>) {
    for (_, dependency) in table.iter() {
        if let Some(path) = dependency
            .as_table_like()
            .and_then(|dependency| dependency.get("path"))
            .and_then(Item::as_str)
        {
            paths.push(path.to_owned());
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::borrow::Cow;
    use std::iter;
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(SourceInputs: Send, Sync, UnwindSafe, RefUnwindSafe);

    #[test]
    fn workspace_reserves_absent_files_and_sources_without_probing_them() {
        let root = Path::new("root");
        let inputs = SourceInputs::default().workspace(
            root,
            &root.join("workspace"),
            &[root.join("workspace/member/Cargo.toml")],
        );
        assert_eq!(
            inputs.files,
            [
                "workspace/Cargo.lock",
                "workspace/.cargo/config",
                "workspace/.cargo/config.toml",
                ".cargo/config",
                ".cargo/config.toml",
                "workspace/member/Cargo.toml",
                "workspace/member/build.rs",
            ]
            .map(|path| root.join(path))
            .into()
        );
        assert_eq!(
            inputs.source_directories,
            [root.join("workspace/member/src")].into()
        );
    }

    #[test]
    fn dependencies_follow_transitive_aliases_and_cycles_without_rereading() {
        let root = Path::new("root");
        let manifests = [root.join("Cargo.toml")];
        let mut reads = Vec::new();
        let inputs = SourceInputs::dependencies_with(
            &manifests,
            |manifest, path| {
                let directory = manifest.parent().unwrap().join(path);
                Ok(if directory == root.join("alias") {
                    root.join("actual")
                } else if directory == root.join("actual/../leaf") {
                    root.join("leaf")
                } else {
                    assert_eq!(directory, root.join("leaf/.."));
                    root.into()
                })
            },
            |path| {
                assert!(!reads.contains(&path.to_path_buf()));
                reads.push(path.to_path_buf());
                parse_document(path, if path == root.join("Cargo.toml") {
                    "[dependencies]\nlocal={path='alias'}\n[target.'cfg(unix)'.build-dependencies]\nlocal={path='alias'}\n[workspace.dependencies]\nlocal={path='alias'}\n[patch.crates-io]\nlocal={path='alias'}\n[replace]\n'local:1.0.0'={path='alias'}\n"
                } else if path == root.join("actual/Cargo.toml") {
                    "[dependencies]\nleaf={path='../leaf'}\n"
                } else {
                    assert_eq!(path, root.join("leaf/Cargo.toml"));
                    "[dependencies]\nroot={path='..'}\n"
                })
            },
        ).unwrap();
        let manifests =
            ["Cargo.toml", "actual/Cargo.toml", "leaf/Cargo.toml"].map(|path| root.join(path));
        assert_eq!(reads, manifests);
        assert_eq!(inputs.files, manifests.into());
        assert_eq!(
            inputs.declared_paths,
            [
                "alias/Cargo.toml",
                "alias/src",
                "actual/../leaf/Cargo.toml",
                "actual/../leaf/src",
                "leaf/../Cargo.toml",
                "leaf/../src",
            ]
            .map(|path| root.join(path))
            .into()
        );
        assert_eq!(
            inputs.source_directories,
            ["src", "actual/src", "leaf/src"]
                .map(|path| root.join(path))
                .into()
        );
        let table: DocumentMut =
            "a = { path = 'first' }\nb = '1'\nc = { git = 'url' }\nd = { path = 'second' }\n"
                .parse()
                .unwrap();
        let mut paths = Vec::new();
        dependency_paths(table.as_table(), &mut paths);
        assert_eq!(paths, ["first", "second"]);
    }

    #[test]
    fn dependencies_share_acquired_documents_but_reacquire_new_inputs_each_pass() {
        let root = PathBuf::from("root/Cargo.toml");
        let captured = parse_document(&root, "[dependencies]\nlocal={path='external'}\n").unwrap();
        for leaf in ["first", "second"] {
            let mut shared = 0;
            let mut reads = Vec::new();
            let inputs = SourceInputs::dependencies_with(
                [&root],
                |_, dependency| Ok(Path::new("root").join(dependency)),
                |path| {
                    if path == root {
                        shared += 1;
                        return Ok(Cow::Borrowed(&captured));
                    }
                    assert!(!reads.contains(&path.to_path_buf()));
                    reads.push(path.to_path_buf());
                    if path == Path::new("root/external/Cargo.toml") {
                        parse_document(path, &format!("[dependencies]\nleaf={{path='{leaf}'}}\n"))
                            .map(Cow::Owned)
                    } else {
                        assert_eq!(path, Path::new("root").join(leaf).join("Cargo.toml"));
                        Ok(Cow::Owned(DocumentMut::new()))
                    }
                },
            )
            .unwrap();
            assert_eq!(shared, 1);
            let acquired = [
                PathBuf::from("root/external/Cargo.toml"),
                Path::new("root").join(leaf).join("Cargo.toml"),
            ];
            assert_eq!(reads, acquired);
            assert_eq!(
                inputs.files,
                iter::once(root.clone()).chain(acquired).collect()
            );
            assert_eq!(
                inputs.source_directories,
                [
                    PathBuf::from("root/external/src"),
                    Path::new("root").join(leaf).join("src"),
                ]
                .into()
            );
        }
    }

    #[test]
    fn manifest_and_dependency_errors_are_not_empty_inventory() {
        for text in [None, Some("["), Some("[dependencies]\nx={path='local'}")] {
            SourceInputs::dependencies_with(
                [&PathBuf::from("Cargo.toml")],
                |_, _| Err(ReadFileError::new(Path::new("local")).into()),
                |path| {
                    let text = text.ok_or_else(|| ReadFileError::new(path))?;
                    parse_document(path, text)
                },
            )
            .unwrap_err();
        }
    }

    #[test]
    fn explicit_sources_cover_target_forms_without_treating_other_paths_as_sources() {
        let root = Path::new("root");
        let document = parse_document(
            &root.join("Cargo.toml"),
            "example=[{path='custom/example.rs'},{name='implicit'}]\n\
             bench=[{path='custom/bench.rs'}]\n\
             [package]\nbuild='custom/build.rs'\n\
             [package.metadata]\npath='not-source'\n\
             [lib]\npath='custom/lib.rs'\n\
             [[bin]]\npath='custom/main.rs'\n\
             [[bin]]\nname='implicit'\n\
             [[test]]\npath='custom/test.rs'\n",
        )
        .unwrap();
        let mut inputs = SourceInputs::default();
        inputs.explicit_sources(root, &document);
        assert_eq!(
            inputs.files,
            ["build", "lib", "main", "example", "test", "bench"]
                .map(|name| root.join(format!("custom/{name}.rs")))
                .into()
        );
        assert!(inputs.source_directories.is_empty());
        let document = parse_document(
            &root.join("Cargo.toml"),
            "[package]\nbuild=false\n[lib]\nname='implicit'\n",
        )
        .unwrap();
        let mut inputs = SourceInputs::default();
        inputs.explicit_sources(root, &document);
        assert!(inputs.files.is_empty());
        for text in ["[package]\n", "[package]\nbuild=true\n"] {
            let mut inputs = SourceInputs::default();
            inputs.explicit_sources(
                root,
                &parse_document(&root.join("Cargo.toml"), text).unwrap(),
            );
            assert_eq!(inputs.files, [root.join("build.rs")].into());
        }
    }

    #[test]
    fn a_reserved_target_does_not_suppress_dependency_document_acquisition() {
        let root = PathBuf::from("root/Cargo.toml");
        let mut reads = Vec::new();
        let inputs = SourceInputs::dependencies_with(
            [&root],
            |manifest, dependency| Ok(manifest.parent().unwrap().join(dependency)),
            |path| {
                reads.push(path.to_path_buf());
                let text = if path == root {
                    "[lib]\npath='dependency/Cargo.toml'\n[dependencies]\nlocal={path='dependency'}"
                } else {
                    "[package]\nbuild='custom-build.rs'"
                };
                parse_document(path, text)
            },
        )
        .unwrap();
        assert_eq!(reads, [root, PathBuf::from("root/dependency/Cargo.toml")]);
        assert!(
            inputs
                .files
                .contains(Path::new("root/dependency/custom-build.rs"))
        );
    }
}
