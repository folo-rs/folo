//! In-process Git observations; repository acquisition belongs to integration fixtures.

use std::path::Path;

use crate::git::{GitRepo, TreeEntry};

/// An inert handle that acquires no repository state.
pub(crate) fn unopened(root: &Path) -> GitRepo {
    GitRepo {
        root: root.to_path_buf(),
        prefix: String::new(),
    }
}

/// Creates a mode-bearing entry without reading its blob.
pub(crate) fn tree_entry(path: &str, mode: &str) -> TreeEntry {
    TreeEntry {
        path: path.to_string(),
        id: "unused-blob".to_string(),
        mode: mode.to_string(),
    }
}
