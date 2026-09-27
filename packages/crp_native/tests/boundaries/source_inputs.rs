use std::fs;
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crp_diag::DiagnosticSink;
use crp_native::{BuildRequest, Native, SourceProvider};
use crp_workspace::testing::{Repository, with_io_slot};
use ohno::AppError;
use tempfile::TempDir;

/// Observes native operation starts without changing process-wide output configuration.
#[derive(Debug, Default)]
struct Recording(Mutex<Vec<String>>);

impl DiagnosticSink for Recording {
    fn write(&self, text: &str) -> io::Result<()> {
        self.0.lock().unwrap().push(text.to_owned());
        Ok(())
    }
}

/// The fixture source exists locally; this test must not contact a remote repository.
struct LocalSource;

impl SourceProvider for LocalSource {
    fn fetch(&self, _controller: &Path, _commit: &str, _deadline: Instant) -> Result<(), AppError> {
        panic!("fixture source already exists")
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "checks committed link modes and native worktree materialization"
)]
fn tracked_toolchain_links_are_rejected_before_rustup_even_when_materialized_as_text() {
    with_io_slot(|| {
        testing::with_watchdog_timeout(Duration::from_mins(5), || {
            let repository = Repository::new();
            repository.write(
                "Cargo.toml",
                b"[workspace]\n[package]\nname='fixture'\nversion='1.0.0'\nedition='2024'\n",
            );
            repository.write(
                "Cargo.lock",
                b"version=4\n[[package]]\nname='fixture'\nversion='1.0.0'\n",
            );
            repository.write("src/main.rs", b"fn main() {}\n");
            repository.write(
                "rust-toolchain.toml",
                include_bytes!("../../../../rust-toolchain.toml"),
            );
            let external = TempDir::new().unwrap();
            let toolchain = external.path().join("external-toolchain.toml");
            fs::write(
                &toolchain,
                include_bytes!("../../../../rust-toolchain.toml"),
            )
            .unwrap();
            repository.write("link-target", toolchain.to_str().unwrap().as_bytes());
            let object = repository
                .command(&["hash-object", "-w", "link-target"])
                .trim()
                .to_owned();
            fs::remove_file(repository.path().join("link-target")).unwrap();
            repository.command(&["add", "."]);
            // The Git mode, not a privileged Windows filesystem operation, creates this link input.
            repository.command(&[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("120000,{object},rust-toolchain.toml"),
            ]);
            repository.command(&["commit", "--quiet", "-m", "linked source toolchain"]);
            let source = repository.command(&["rev-parse", "HEAD"]).trim().to_owned();
            // Controller metadata uses a normal toolchain; the frozen historical source has the link.
            repository.command(&["update-index", "--force-remove", "rust-toolchain.toml"]);
            repository.command(&["add", "rust-toolchain.toml"]);
            repository.command(&["commit", "--quiet", "-m", "regular controller toolchain"]);
            repository.command(&["config", "core.symlinks", "false"]);
            let artifacts = TempDir::new().unwrap();
            let recording = Arc::new(Recording::default());
            let sink: Arc<dyn DiagnosticSink> = Arc::<Recording>::clone(&recording);
            let mut native = Native::new(
                repository.path().to_owned(),
                artifacts.path().to_owned(),
                "native".to_owned(),
                Box::new(LocalSource),
                sink,
            )
            .unwrap();
            let request = BuildRequest::new(
                "fixture".to_owned(),
                "fixture".to_owned(),
                "1.0.0".to_owned(),
                "fixture-v1.0.0".to_owned(),
                source,
                "fixture-native".to_owned(),
            )
            .unwrap();
            native.prepare(&request).unwrap_err();
            assert!(
                !recording
                    .0
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|line| line.starts_with("Running rustup "))
            );
            native.cleanup().unwrap();
            assert_eq!(
                repository
                    .command(&["worktree", "list", "--porcelain"])
                    .matches("worktree ")
                    .count(),
                1
            );
        });
    });
}
