use std::path::PathBuf;
use std::sync::Arc;

use crp_native::{BuildRequest, Native};
use ohno::AppError;

use crate::publication::binaries::{Asset, Binary, Executor, Github};

/// Composes native execution with publication-owned asset delivery and verification.
#[derive(Debug)]
pub struct BinaryPublisher {
    native: Native,
    github: Github,
    target: String,
}

impl BinaryPublisher {
    // Source discovery and build-target acquisition are integration boundaries.
    #[cfg_attr(test, mutants::skip)]
    pub fn new(
        controller: PathBuf,
        output: PathBuf,
        target: String,
        github: Github,
    ) -> Result<Self, AppError> {
        let native = Native::new(
            controller,
            output,
            target.clone(),
            Box::new(github.clone()),
            Arc::clone(github.output.diagnostics()),
        )?;
        Ok(Self {
            native,
            github,
            target,
        })
    }
    fn build_request(&self, binary: &Binary) -> Result<BuildRequest, AppError> {
        binary.validate()?;
        BuildRequest::new(
            binary.name.clone(),
            binary.bin.clone(),
            binary.version.clone(),
            binary.tag.clone(),
            binary.source_sha.clone(),
            binary.archive_base(&self.target),
        )
    }
}

impl Executor for BinaryPublisher {
    fn cancelled(&self) -> bool {
        self.native.cancelled()
    }

    #[cfg_attr(test, mutants::skip)] // Native/remote adapter wiring has boundary coverage.
    fn assets(&mut self, binary: &Binary) -> Result<Vec<Asset>, AppError> {
        self.github
            .assets(binary, self.native.execution_context().directory)
    }

    #[cfg_attr(test, mutants::skip)] // Native source acquisition has boundary coverage.
    fn prepare(&mut self, binary: &Binary) -> Result<(), AppError> {
        self.native.prepare(&self.build_request(binary)?)
    }

    #[cfg_attr(test, mutants::skip)] // Native Cargo execution has boundary coverage.
    fn build(&mut self, binary: &Binary) -> Result<(), AppError> {
        self.native.build(&self.build_request(binary)?)
    }

    #[cfg_attr(test, mutants::skip)] // Native archive execution has boundary coverage.
    fn package(&mut self, binary: &Binary) -> Result<(), AppError> {
        self.native.package(&self.build_request(binary)?)
    }

    #[cfg_attr(test, mutants::skip)] // Explicit GitHub adapter, never a live write in tests.
    fn upload(&mut self, binary: &Binary) -> Result<(), AppError> {
        let artifacts = self.native.artifacts()?;
        let context = self.native.execution_context();
        self.github.invoke(
            &[
                "release".into(),
                "upload".into(),
                binary.tag.clone().into(),
                artifacts.archive.as_os_str().to_owned(),
                artifacts.checksum.as_os_str().to_owned(),
                "--clobber".into(),
            ],
            context.directory,
            context.deadline,
        )?;
        if !binary.complete(
            &self.target,
            &self
                .github
                .assets_until(binary, context.directory, context.deadline)?,
        ) {
            return Err(IncompleteAssetUpload::new(binary.tag.clone(), self.target.clone()).into());
        }

        Ok(())
    }

    #[cfg_attr(test, mutants::skip)] // Owned native resource cleanup has boundary coverage.
    fn cleanup(&mut self) -> Result<(), AppError> {
        self.native.cleanup()
    }
}

/// A successful upload command did not establish the required remote pair.
#[ohno::error]
#[display("upload for {tag} on {target} returned success without both uploaded assets")]
struct IncompleteAssetUpload {
    tag: String,
    target: String,
}
