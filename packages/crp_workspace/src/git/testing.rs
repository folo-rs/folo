//! In-process Git observations; repository acquisition belongs to integration fixtures.

use std::path::Path;

use crate::git::{GitObjectContext, GitRepo, TreeEntry};

/// Creates an inert interpretation context without querying a repository.
#[must_use]
pub fn object_context(format: &str) -> GitObjectContext {
    GitObjectContext::for_test(format)
}

/// An inert handle that acquires no repository state.
#[must_use]
pub fn unopened(root: &Path) -> GitRepo {
    GitRepo {
        root: root.to_path_buf(),
        prefix: String::new(),
    }
}

/// Creates a mode-bearing entry without reading its blob.
#[must_use]
pub fn tree_entry(path: &str, mode: &str) -> TreeEntry {
    TreeEntry {
        path: path.to_string(),
        id: "unused-blob".to_string(),
        mode: mode.to_string(),
    }
}
