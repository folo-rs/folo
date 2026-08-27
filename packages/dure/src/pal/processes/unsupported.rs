//! Process PAL used on non-Windows builds.

use std::path::PathBuf;

use crate::pal::error::{PalError, PalErrorKind};
use crate::pal::ids::{AppId, JobId};
use crate::pal::processes::{AppSpawn, ProcessLiveness, Processes, SupervisorSpawn};
use crate::session_record::ProcessIdentity;

/// Stub process control. The platform gate refuses to run before this is used.
#[derive(Debug, Default)]
pub(crate) struct BuildTargetProcesses;

fn unsupported<T>() -> Result<T, PalError> {
    Err(PalError::new(PalErrorKind::Other))
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)]
impl Processes for BuildTargetProcesses {
    fn current_exe(&self) -> Result<PathBuf, PalError> {
        unsupported()
    }

    fn spawn_supervisor(&self, _request: &SupervisorSpawn) -> Result<ProcessIdentity, PalError> {
        unsupported()
    }

    fn probe(&self, _identity: &ProcessIdentity) -> ProcessLiveness {
        ProcessLiveness::InspectFailed
    }

    fn terminate(&self, _identity: &ProcessIdentity) -> Result<(), PalError> {
        unsupported()
    }

    fn create_lifetime_job(&self) -> Result<JobId, PalError> {
        unsupported()
    }

    fn close_job(&self, _job: JobId) {}

    fn spawn_app(&self, _request: &AppSpawn) -> Result<AppId, PalError> {
        unsupported()
    }

    fn wait_app(&self, _app: AppId) -> Result<i32, PalError> {
        unsupported()
    }

    fn current_identity(&self) -> Result<ProcessIdentity, PalError> {
        unsupported()
    }

    fn random_nonce(&self) -> String {
        String::new()
    }
}
