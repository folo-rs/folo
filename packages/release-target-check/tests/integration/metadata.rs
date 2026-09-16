use std::fs;
#[cfg(unix)]
use std::io::Error;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::Path;

use release_target_check::Metadata;
use serde_json::json;

use crate::repository_fixture::{command, fixture};
use crate::scheduling::with_io_test;

fn sample(root: &Path, manifest: &Path) -> Metadata {
    let input = json!({
        "workspace_root": root,
        "workspace_members": ["widget-id"],
        "packages": [{
            "name": "widget",
            "version": "1.0.0",
            "id": "widget-id",
            "manifest_path": manifest,
            "publish": null
        }]
    });
    Metadata::parse(&serde_json::to_vec(&input).unwrap()).unwrap()
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git against filesystem fixtures")]
fn validates_tracked_inputs_with_and_without_a_lockfile() {
    with_io_test(|| {
        let (_directory, repository) = fixture();
        let manifest = repository.root().join("Cargo.toml");
        let metadata = sample(repository.root(), &manifest);
        metadata.validate_inputs(&repository, &manifest).unwrap();

        fs::write(repository.root().join("Cargo.lock"), "tracked lockfile").unwrap();
        command(repository.root(), &["add", "Cargo.lock"]);
        metadata.validate_inputs(&repository, &manifest).unwrap();
    });
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git against filesystem fixtures")]
fn rejects_an_untracked_lockfile_or_member_manifest() {
    with_io_test(|| {
        let (_directory, repository) = fixture();
        let manifest = repository.root().join("Cargo.toml");
        let metadata = sample(repository.root(), &manifest);
        let lockfile = repository.root().join("Cargo.lock");
        fs::write(&lockfile, "untracked lockfile").unwrap();
        _ = metadata
            .validate_inputs(&repository, &manifest)
            .unwrap_err();

        fs::remove_file(lockfile).unwrap();
        let member = repository.root().join("member.toml");
        fs::write(&member, "untracked member manifest").unwrap();
        let metadata = sample(repository.root(), &member);
        _ = metadata
            .validate_inputs(&repository, &manifest)
            .unwrap_err();
    });
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git against filesystem fixtures")]
fn rejects_a_manifest_outside_the_reported_workspace() {
    with_io_test(|| {
        let (_directory, repository) = fixture();
        let nested = repository.root().join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("Cargo.toml"), "nested workspace").unwrap();
        command(repository.root(), &["add", "nested/Cargo.toml"]);
        let metadata = sample(&nested, &nested.join("Cargo.toml"));
        _ = metadata
            .validate_inputs(&repository, &repository.root().join("Cargo.toml"))
            .unwrap_err();
    });
}

#[cfg(unix)]
#[test]
#[cfg_attr(miri, ignore = "Creates a filesystem symlink loop and executes Git")]
fn propagates_a_lockfile_lookup_error() {
    with_io_test(|| {
        let (_directory, repository) = fixture();
        // An owned symlink loop produces a deterministic lookup error without races or
        // permission assumptions that change when a runner is privileged.
        symlink("Cargo.lock", repository.root().join("Cargo.lock")).unwrap();
        let manifest = repository.root().join("Cargo.toml");
        let metadata = sample(repository.root(), &manifest);
        let error = metadata
            .validate_inputs(&repository, &manifest)
            .unwrap_err();
        assert!(error.find_source::<Error>().is_some());
    });
}
