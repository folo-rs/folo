use std::ffi::OsString;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tempfile::TempDir;
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::commands::setup_azure::bundle::BundleFile;

/// Owns only bundle filesystem operations; caller directories are never deleted.
pub(crate) trait BundleFiles {
    type Temporary: AsRef<Path>;

    fn export(&self, path: &Path, files: &[BundleFile]) -> impl Future<Output = io::Result<()>>;
    fn temporary(&self) -> impl Future<Output = io::Result<Self::Temporary>>;
    fn populate(&self, path: &Path, files: &[BundleFile]) -> impl Future<Output = io::Result<()>>;
    fn cleanup(&self, directory: Self::Temporary) -> impl Future<Output = io::Result<()>>;
}

/// Captured child output, portable to in-process fakes without an OS exit status.
#[derive(Debug)]
pub(crate) struct ProcessOutput {
    pub(crate) success: bool,
    pub(crate) code: Option<i32>,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

/// Executes PowerShell with structural argv, never executable user-supplied text.
pub(crate) trait SetupProcess {
    fn run(&self, arguments: &[OsString]) -> impl Future<Output = io::Result<ProcessOutput>>;
}

/// Real Tokio filesystem adapter with uniquely owned temporary-directory cleanup.
pub(crate) struct TokioBundleFiles;

impl BundleFiles for TokioBundleFiles {
    type Temporary = TempDir;

    // Filesystem ownership and create-new behavior are exercised by native
    // integration tests, outside the library-only mutation target.
    #[cfg_attr(test, mutants::skip)]
    async fn export(&self, path: &Path, files: &[BundleFile]) -> io::Result<()> {
        fs::create_dir_all(path).await?;
        if fs::read_dir(path).await?.next_entry().await?.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "export destination must be absent or empty",
            ));
        }
        self.populate(path, files).await
    }

    // The OS temporary-directory adapter is covered through the native binary.
    #[cfg_attr(test, mutants::skip)]
    async fn temporary(&self) -> io::Result<TempDir> {
        tokio::task::spawn_blocking(|| {
            tempfile::Builder::new()
                .prefix("cargo-bench-history-azure-")
                .tempdir()
        })
        .await?
    }

    // Actual exclusive file creation belongs to native integration coverage.
    #[cfg_attr(test, mutants::skip)]
    async fn populate(&self, path: &Path, files: &[BundleFile]) -> io::Result<()> {
        for file in files {
            // create_new also protects against a competing writer after the empty
            // directory check; exports never overwrite even an individual file.
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path.join(file.name))
                .await?;
            output.write_all(file.contents.as_bytes()).await?;
            output.flush().await?;
        }
        Ok(())
    }

    // Native binary tests verify removal leaves caller-owned siblings intact.
    #[cfg_attr(test, mutants::skip)]
    async fn cleanup(&self, directory: TempDir) -> io::Result<()> {
        tokio::task::spawn_blocking(move || directory.close()).await?
    }
}

/// Captures the one standalone driver's diagnostics without an interactive shell.
pub(crate) struct TokioSetupProcess {
    pub(crate) working_directory: PathBuf,
}

impl SetupProcess for TokioSetupProcess {
    // Process creation and captured native diagnostics require integration tests.
    #[cfg_attr(test, mutants::skip)]
    async fn run(&self, arguments: &[OsString]) -> io::Result<ProcessOutput> {
        let output = Command::new("pwsh")
            .args(arguments)
            .current_dir(&self.working_directory)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output()
            .await?;
        Ok(ProcessOutput {
            success: output.status.success(),
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}
