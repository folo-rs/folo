use std::io;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use cbh_diag::RecordingReporter;
use cbh_storage::{MemoryStorage, Storage, StorageError};
use futures::executor::block_on;
use ohno::AppError;

use super::*;
use crate::commands::collect::{CollectSummary, Partition};
use crate::errors::{
    AddWorktreeFailedError, BenchFailure, FirstParentWalkFailedError, MissingProjectDirectoryError,
    ParseOutputError, RemoveWorktreeFailedError, ResetWorktreeFailedError, ResolveRefFailedError,
};
use crate::{BackfillError, BackfillOptions, DuplicateResultError, EngineFailedError, RunOutcome};

mod bounded;
mod execution;
mod fixtures;
mod outcomes;
mod planning;
mod recording;

use fixtures::*;
