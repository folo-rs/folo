//! Source locations shared by captured evidence and disposable-cache admission.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use ohno::AppError;
use toml_edit::{Item, TableLike};

use crate::ReadFileError;
use crate::manifest::{for_each_dependency_table, parse_document};

/// Discovers reserved files and recursively acquired source directories.
///
/// Paths retain their acquired spelling. Callers add Git-tracked files and decide whether
/// dependency locations are relocatable; cache admission also protects nonrelocatable sources.
#[derive(Debug, Default)]
pub struct SourceInputs {
    pub files: BTreeSet<PathBuf>,
    pub source_directories: BTreeSet<PathBuf>,
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
        let inputs = Self::dependencies(
            manifests.iter().chain([&workspace_root.join("Cargo.toml")]),
            resolve_dependency,
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
        Self::dependencies_with(manifests, resolve_dependency, |path| {
            fs::read_to_string(path).map_err(|error| ReadFileError::caused_by(path, error).into())
        })
    }

    fn dependencies_with<'a>(
        manifests: impl IntoIterator<Item = &'a PathBuf>,
        mut resolve_dependency: impl FnMut(&Path, &Path) -> Result<PathBuf, AppError>,
        mut read: impl FnMut(&Path) -> Result<String, AppError>,
    ) -> Result<Self, AppError> {
        let mut inputs = Self::default();
        let mut pending: BTreeSet<PathBuf> = manifests.into_iter().cloned().collect();
        while let Some(manifest) = pending.pop_first() {
            if !inputs.files.insert(manifest.clone()) {
                continue;
            }
            let text = read(&manifest)?;
            let document = parse_document(&manifest, &text)?;
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
                let directory = resolve_dependency(&manifest, Path::new(&dependency))?;
                inputs.source_directories.insert(directory.join("src"));
                pending.insert(directory.join("Cargo.toml"));
            }
        }
        Ok(inputs)
    }
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
                Ok(if path == root.join("Cargo.toml") {
                    "[dependencies]\nlocal={path='alias'}\n[target.'cfg(unix)'.build-dependencies]\nlocal={path='alias'}\n[workspace.dependencies]\nlocal={path='alias'}\n[patch.crates-io]\nlocal={path='alias'}\n[replace]\n'local:1.0.0'={path='alias'}\n"
                } else if path == root.join("actual/Cargo.toml") {
                    "[dependencies]\nleaf={path='../leaf'}\n"
                } else {
                    assert_eq!(path, root.join("leaf/Cargo.toml"));
                    "[dependencies]\nroot={path='..'}\n"
                }.into())
            },
        ).unwrap();
        let manifests =
            ["Cargo.toml", "actual/Cargo.toml", "leaf/Cargo.toml"].map(|path| root.join(path));
        assert_eq!(reads, manifests);
        assert_eq!(inputs.files, manifests.into());
        assert_eq!(
            inputs.source_directories,
            ["src", "actual/src", "leaf/src"]
                .map(|path| root.join(path))
                .into()
        );
        let table: toml_edit::DocumentMut =
            "a = { path = 'first' }\nb = '1'\nc = { git = 'url' }\nd = { path = 'second' }\n"
                .parse()
                .unwrap();
        let mut paths = Vec::new();
        dependency_paths(table.as_table(), &mut paths);
        assert_eq!(paths, ["first", "second"]);
    }

    #[test]
    fn manifest_and_dependency_errors_are_not_empty_inventory() {
        for text in [None, Some("["), Some("[dependencies]\nx={path='local'}")] {
            SourceInputs::dependencies_with(
                [&PathBuf::from("Cargo.toml")],
                |_, _| Err(ReadFileError::new(Path::new("local")).into()),
                |_| {
                    text.map(str::to_owned)
                        .ok_or_else(|| ReadFileError::new(Path::new("Cargo.toml")).into())
                },
            )
            .unwrap_err();
        }
    }
}
