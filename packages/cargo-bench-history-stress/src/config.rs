//! Persists the rendered analysis configuration in the harness workspace.

use std::io;
use std::path::{Path, PathBuf};

use crate::error::{Error, fail};
use crate::measure::config_path;

/// Writes rendered configuration into the workspace and returns its path.
///
/// Creates the parent directories and replaces any existing file contents.
///
/// # Errors
///
/// Returns an error if directory creation or file writing fails.
// This nonpublished library exposes the real adapter for Cargo integration tests.
// Only Tokio filesystem delegation is excluded from mutation testing; ordering,
// destination selection and error forwarding stay in the in-process writer.
// Ref: docs/implementation.md, "Configuration writes".
#[cfg_attr(test, mutants::skip)]
pub async fn write_config(workspace: &Path, contents: &str) -> Result<PathBuf, Error> {
    write_config_with(
        workspace,
        contents,
        async |parent| tokio::fs::create_dir_all(parent).await,
        async |path, contents| tokio::fs::write(path, contents).await,
    )
    .await
}

/// Sequences configuration operations without selecting a filesystem implementation.
async fn write_config_with(
    workspace: &Path,
    contents: &str,
    create_parent: impl AsyncFn(&Path) -> io::Result<()>,
    write: impl AsyncFn(&Path, &str) -> io::Result<()>,
) -> Result<PathBuf, Error> {
    let path = config_path(workspace);
    if let Some(parent) = path.parent() {
        create_parent(parent)
            .await
            .map_err(|error| fail(format!("failed to create {}: {error}", parent.display())))?;
    }
    write(&path, contents)
        .await
        .map_err(|error| fail(format!("failed to write {}: {error}", path.display())))?;
    Ok(path)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::{Cell, RefCell};

    use futures::executor::block_on;

    use super::*;

    #[test]
    fn writes_exact_contents_after_creating_the_configuration_parent() {
        let workspace = Path::new("workspace");
        let parent = workspace.join(".cargo");
        let path = parent.join("bench_history.toml");
        let created = RefCell::new(None);
        let written = RefCell::new(None);
        // Escaped bytes and trailing newlines must reach storage without transformation.
        let contents = "[project]\nid = \"quoted\\\"project\"\n";

        let result = block_on(write_config_with(
            workspace,
            contents,
            async |requested| {
                assert!(created.borrow().is_none());
                assert!(written.borrow().is_none());
                *created.borrow_mut() = Some(requested.to_path_buf());
                Ok(())
            },
            async |requested, contents| {
                assert_eq!(created.borrow().as_deref(), Some(parent.as_path()));
                assert!(written.borrow().is_none());
                *written.borrow_mut() = Some((requested.to_path_buf(), contents.to_owned()));
                Ok(())
            },
        ))
        .unwrap();

        assert_eq!(result, path);
        assert_eq!(*written.borrow(), Some((path, contents.to_owned())));
    }

    #[test]
    fn forwards_empty_contents_to_the_writer() {
        let called = Cell::new(false);
        _ = block_on(write_config_with(
            Path::new("workspace"),
            "",
            async |_| Ok(()),
            async |_, contents| {
                assert!(contents.is_empty());
                called.set(true);
                Ok(())
            },
        ))
        .unwrap();
        assert!(called.get());
    }

    #[test]
    fn parent_failure_is_forwarded_without_attempting_a_write() {
        let workspace = Path::new("workspace");
        let error = block_on(write_config_with(
            workspace,
            "configuration",
            async |_| Err(io::Error::other("parent-canary")),
            async |_, _| panic!("a failed parent operation must stop the write"),
        ))
        .unwrap_err();

        // Verify the injected cause and attempted destination, not diagnostic wording.
        assert!(error.to_string().contains("parent-canary"));
        assert!(
            error
                .to_string()
                .contains(&workspace.join(".cargo").display().to_string())
        );
    }

    #[test]
    fn write_failure_is_forwarded_after_parent_creation() {
        let workspace = Path::new("workspace");
        let created = Cell::new(false);
        let error = block_on(write_config_with(
            workspace,
            "configuration",
            async |_| {
                created.set(true);
                Ok(())
            },
            async |_, _| {
                assert!(created.get());
                Err(io::Error::other("write-canary"))
            },
        ))
        .unwrap_err();

        assert!(error.to_string().contains("write-canary"));
        assert!(
            error.to_string().contains(
                &workspace
                    .join(".cargo")
                    .join("bench_history.toml")
                    .display()
                    .to_string()
            )
        );
    }
}
