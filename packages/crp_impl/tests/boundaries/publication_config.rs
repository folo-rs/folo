//! Workspace-relative publication configuration and Git branch-name validation.

use std::path::Path;

use crp_impl::publication::config::{Configuration, NativeTarget};

use crate::git_fixture::Repository;

const CONFIG: &[u8] = b"schema-version = 1\nrepository = 'example/tools'\n\
                       release-branch = 'stable/releases'\n\
                       targets = ['x86_64-unknown-linux-gnu']\n";

#[test]
#[cfg_attr(miri, ignore = "Reads configuration and invokes Git")]
fn loads_default_and_overridden_paths_from_the_selected_workspace() {
    let fixture = Repository::new();
    fixture.write("nested/.cargo/release_plan.toml", CONFIG);
    let workspace = fixture.path().join("nested");
    let (path, config) = Configuration::load(&workspace, None).unwrap();
    assert_eq!(path, workspace.join(".cargo/release_plan.toml"));
    assert_eq!(config.repository(), "example/tools");
    assert_eq!(config.release_branch(), "stable/releases");
    assert_eq!(
        config.binary_targets("tool", None).unwrap(),
        [NativeTarget::LinuxX64]
    );

    fixture.write("nested/alternate.toml", CONFIG);
    let (_, relative) = Configuration::load(&workspace, Some(Path::new("alternate.toml"))).unwrap();
    let (_, absolute) =
        Configuration::load(&workspace, Some(&workspace.join("alternate.toml"))).unwrap();
    assert_eq!(relative, config);
    assert_eq!(absolute, config);
}

#[test]
#[cfg_attr(miri, ignore = "Reads configuration and invokes Git")]
fn rejects_missing_configuration_and_invalid_git_branch_names() {
    let fixture = Repository::new();
    Configuration::load(fixture.path(), None).unwrap_err();
    fixture.write(
        ".cargo/release_plan.toml",
        b"schema-version = 1\nrepository = 'example/tools'\n\
          release-branch = 'invalid..branch'\ntargets = []\n",
    );
    Configuration::load(fixture.path(), None).unwrap_err();
}
