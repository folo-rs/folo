use std::path::PathBuf;
use std::sync::Arc;

use crp_native::{BuildRequest, Native};
use ohno::AppError;

use crate::publication::binaries::batch::Executor;
use crate::publication::binaries::github::Github;
use crate::publication::binaries::model::{Asset, Binary, InvalidPlan};

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
}

impl Binary {
    pub(crate) fn build_request(&self, target: &str) -> Result<BuildRequest, AppError> {
        self.validate()?;
        BuildRequest::new(
            self.name.clone(),
            self.bin.clone(),
            self.version.clone(),
            self.tag.clone(),
            self.source_sha.clone(),
            self.archive_base(target),
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
        self.native.prepare(&binary.build_request(&self.target)?)
    }

    #[cfg_attr(test, mutants::skip)] // Native Cargo execution has boundary coverage.
    fn build(&mut self, binary: &Binary) -> Result<(), AppError> {
        self.native.build(&binary.build_request(&self.target)?)
    }

    #[cfg_attr(test, mutants::skip)] // Native archive execution has boundary coverage.
    fn package(&mut self, binary: &Binary) -> Result<(), AppError> {
        self.native.package(&binary.build_request(&self.target)?)
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
            return Err(InvalidPlan::new(
                "Upload returned success without both uploaded assets".to_owned(),
            )
            .into());
        }
        Ok(())
    }

    #[cfg_attr(test, mutants::skip)] // Owned native resource cleanup has boundary coverage.
    fn cleanup(&mut self) -> Result<(), AppError> {
        self.native.cleanup()
    }
}
