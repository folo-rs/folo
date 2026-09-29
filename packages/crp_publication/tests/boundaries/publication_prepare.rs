//! Preparation freezes source inputs without predicting whether Cargo can resolve them.

use std::fs;
use std::process::Command;
use std::sync::Arc;

use crp_diag::Discard;
use crp_publication::PublicationOutput;
use crp_publication::publication::manifest::PublicationManifest;
use crp_publication::publication::prepare::prepare;

use crate::git_fixture::Repository;
use crate::with_io_test;

#[test]
#[cfg_attr(miri, ignore = "Uses Git and Cargo against an owned local workspace")]
fn preparation_preserves_a_recorded_lockfile_that_the_locked_build_rejects() {
    with_io_test(|| {
        let repository = source();
        let commit = repository.command(&["rev-parse", "HEAD"]).trim().to_owned();
        let lock = fs::read(repository.path().join("Cargo.lock")).unwrap();
        let artifacts = tempfile::tempdir().unwrap();
        let manifest = repository.path().join("Cargo.toml");
        let output = artifacts.path().join("publication.json");
        prepare(
            &manifest,
            None,
            &commit,
            &output,
            &PublicationOutput::new("1.0.0", false, Arc::new(Discard)),
        )
        .unwrap();
        let publication = PublicationManifest::read(&output).unwrap();
        assert_eq!(publication.publication.source, commit);
        assert_eq!(publication.publication.packages.len(), 1);
        assert_eq!(
            publication.publication.packages.first().unwrap().name,
            "library"
        );

        // Only local path dependencies exist. Cargo's explicit locked build is the
        // authoritative resolution failure, and neither operation may repair the lockfile.
        let build = Command::new("cargo")
            .args([
                "build",
                "--offline",
                "--locked",
                "--package",
                "library",
                "--lib",
            ])
            .env("CARGO_TARGET_DIR", artifacts.path().join("target"))
            .current_dir(repository.path())
            .output()
            .unwrap();
        assert!(!build.status.success());
        assert!(String::from_utf8_lossy(&build.stderr).contains("--locked"));
        assert_eq!(
            fs::read(repository.path().join("Cargo.lock")).unwrap(),
            lock
        );
        assert!(repository.command(&["status", "--porcelain"]).is_empty());
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Uses Git and filesystem mutations in an owned workspace"
)]
fn preparation_still_requires_a_tracked_unchanged_lockfile() {
    with_io_test(|| {
        for untracked in [false, true] {
            let repository = source();
            if untracked {
                repository.command(&["rm", "--cached", "Cargo.lock"]);
                repository.command(&["commit", "--quiet", "-m", "untracked lockfile"]);
                repository.command(&["branch", "--force", "release", "HEAD"]);
            } else {
                repository.write("Cargo.lock", b"version = 4\n# changed source input\n");
            }
            let commit = repository.command(&["rev-parse", "HEAD"]).trim().to_owned();
            let lock = fs::read(repository.path().join("Cargo.lock")).unwrap();
            let artifacts = tempfile::tempdir().unwrap();
            let output = artifacts.path().join("publication.json");
            let error = prepare(
                &repository.path().join("Cargo.toml"),
                None,
                &commit,
                &output,
                &PublicationOutput::new("1.0.0", false, Arc::new(Discard)),
            )
            .unwrap_err();
            if untracked {
                assert!(error.to_string().contains("Cargo.lock"));
            }
            assert!(!output.exists());
            assert_eq!(
                fs::read(repository.path().join("Cargo.lock")).unwrap(),
                lock
            );
        }
    });
}

fn source() -> Repository {
    let repository = Repository::new();
    repository.write(
        "Cargo.toml",
        b"[workspace]\nmembers=['library','support']\nresolver='3'\n",
    );
    repository.write(
        "library/Cargo.toml",
        b"[package]\nname='library'\nversion='1.0.0'\nedition='2024'\n\
          [dependencies]\nsupport={path='../support',version='=1.0.0'}\n",
    );
    repository.write(
        "support/Cargo.toml",
        b"[package]\nname='support'\nversion='1.0.0'\nedition='2024'\npublish=false\n",
    );
    repository.write(
        "library/src/lib.rs",
        b"pub fn value() -> u8 { support::value() }\n",
    );
    repository.write("support/src/lib.rs", b"pub fn value() -> u8 { 7 }\n");
    // A library's lockfile is not released content. Omitting its dependency edge isolates
    // Cargo resolvability from the tracked-source and release-content checks under test.
    repository.write(
        "Cargo.lock",
        b"version=4\n[[package]]\nname='library'\nversion='1.0.0'\n\
          [[package]]\nname='support'\nversion='1.0.0'\n",
    );
    repository.write(".gitignore", b"/Cargo.lock\n");
    repository.write(
        ".cargo/release_plan.toml",
        b"schema-version=1\nrepository='example/preparation'\nrelease-branch='release'\ntargets=[]\n",
    );
    repository.command(&["add", "."]);
    repository.command(&["add", "--force", "Cargo.lock"]);
    repository.command(&["commit", "--quiet", "-m", "recorded publication source"]);
    repository.command(&["branch", "release"]);
    repository.command(&[
        "config",
        &format!("url.{}.insteadOf", repository.path().display()),
        "https://github.com/example/preparation.git",
    ]);
    repository
}
