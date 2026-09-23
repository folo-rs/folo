use std::collections::BTreeMap;

use ohno::AppError;
use serde::Serialize;

use crate::model::{Asset, Batch, Binary};

/// Native operations used by the in-process batch sequence.
pub(crate) trait Executor {
    fn cancelled(&self) -> bool;
    fn assets(&mut self, binary: &Binary) -> Result<Vec<Asset>, AppError>;
    fn prepare(&mut self, binary: &Binary) -> Result<(), AppError>;
    fn build(&mut self, binary: &Binary) -> Result<(), AppError>;
    fn package(&mut self, binary: &Binary) -> Result<(), AppError>;
    fn upload(&mut self, binary: &Binary) -> Result<(), AppError>;
    fn cleanup(&mut self) -> Result<(), AppError>;
}

/// A retained item result makes partial success visible even when the job fails.
#[derive(Debug, Serialize)]
pub(crate) struct Outcome {
    pub(crate) binary: Binary,
    pub(crate) status: &'static str,
    pub(crate) stage: &'static str,
    pub(crate) diagnostic: Option<String>,
    pub(crate) cleanup_error: Option<String>,
}

pub(crate) fn execute(
    batch: &Batch,
    no_upload: bool,
    executor: &mut impl Executor,
) -> Result<Vec<Outcome>, AppError> {
    batch.validate()?;
    let mut outcomes = Vec::new();
    let mut sources = BTreeMap::<&str, Vec<&Binary>>::new();
    for binary in &batch.binaries {
        if executor.cancelled() {
            outcomes.push(outcome(binary, "unattempted", "cancelled", None));
            continue;
        }
        if !no_upload {
            match executor.assets(binary) {
                Ok(assets) if binary.complete(&batch.triple, &assets) => {
                    outcomes.push(outcome(binary, "skipped-complete", "refresh", None));
                    continue;
                }
                Ok(_) => {}
                Err(error) => {
                    outcomes.push(failure(binary, "refresh", &error));
                    continue;
                }
            }
        }
        sources.entry(&binary.source_sha).or_default().push(binary);
    }
    for binaries in sources.values() {
        let first = binaries
            .first()
            .expect("only nonempty source groups are inserted");
        if executor.cancelled() {
            for binary in binaries {
                outcomes.push(outcome(binary, "unattempted", "cancelled", None));
            }
            continue;
        }
        match executor.prepare(first) {
            Ok(()) => {
                for binary in binaries {
                    if executor.cancelled() {
                        outcomes.push(outcome(binary, "unattempted", "cancelled", None));
                        continue;
                    }
                    let result = execute_item(binary, no_upload, executor);
                    match result {
                        Ok(()) => outcomes.push(outcome(
                            binary,
                            if no_upload {
                                "staged-only"
                            } else {
                                "published"
                            },
                            if no_upload { "package" } else { "upload" },
                            None,
                        )),
                        Err((stage, error)) => outcomes.push(failure(binary, stage, &error)),
                    }
                }
            }
            Err(error) => {
                for binary in binaries {
                    outcomes.push(failure(binary, "source", &error));
                }
            }
        }
        if let Err(error) = executor.cleanup() {
            eprintln!("Source {} cleanup failed: {error}", first.source_sha);
            for outcome in &mut outcomes {
                if outcome.binary.source_sha == first.source_sha {
                    outcome.cleanup_error = Some(error.to_string());
                }
            }
        }
    }
    Ok(outcomes)
}

fn execute_item(
    binary: &Binary,
    no_upload: bool,
    executor: &mut impl Executor,
) -> Result<(), (&'static str, AppError)> {
    executor.build(binary).map_err(|error| ("build", error))?;
    check_cancellation(executor)?;
    executor
        .package(binary)
        .map_err(|error| ("package", error))?;
    check_cancellation(executor)?;
    if !no_upload {
        executor.upload(binary).map_err(|error| ("upload", error))?;
    }
    Ok(())
}

fn check_cancellation(executor: &impl Executor) -> Result<(), (&'static str, AppError)> {
    if executor.cancelled() {
        return Err((
            "cancelled",
            crate::model::InvalidPlan::new("Release batch cancelled".to_owned()).into(),
        ));
    }
    Ok(())
}

fn failure(binary: &Binary, stage: &'static str, error: &AppError) -> Outcome {
    eprintln!(
        "{} ({}) failed during {stage}: {error}",
        binary.tag, binary.source_sha
    );
    outcome(binary, "failed", stage, Some(error.to_string()))
}

fn outcome(
    binary: &Binary,
    status: &'static str,
    stage: &'static str,
    diagnostic: Option<String>,
) -> Outcome {
    Outcome {
        binary: binary.clone(),
        status,
        stage,
        diagnostic,
        cleanup_error: None,
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::indexing_slicing,
    reason = "Test fixtures specify every indexed item"
)]
mod tests {
    use super::*;
    use crate::model::tests::binary;
    use crate::model::{InvalidPlan, timeout_minutes};

    /// Records ordered native operations and injects a single chosen stage failure.
    #[derive(Default)]
    struct Fake {
        calls: Vec<String>,
        fail: String,
        complete: bool,
        cancel_after: String,
    }

    impl Fake {
        fn call(&mut self, stage: &str, name: &str) -> Result<(), AppError> {
            let call = format!("{stage}:{name}");
            self.calls.push(call.clone());
            if self.fail == call {
                return Err(InvalidPlan::new("injected failure".to_owned()).into());
            }
            Ok(())
        }
    }

    impl Executor for Fake {
        fn cancelled(&self) -> bool {
            !self.cancel_after.is_empty() && self.calls.contains(&self.cancel_after)
        }
        fn assets(&mut self, binary: &Binary) -> Result<Vec<Asset>, AppError> {
            self.call("refresh", &binary.name)?;
            Ok(if self.complete {
                ["zip", "sha256"]
                    .map(|extension| Asset {
                        name: format!("{}.{extension}", binary.archive_base("native")),
                        state: "uploaded".into(),
                    })
                    .into()
            } else {
                vec![]
            })
        }
        fn prepare(&mut self, binary: &Binary) -> Result<(), AppError> {
            self.call("source", &binary.name)
        }
        fn build(&mut self, binary: &Binary) -> Result<(), AppError> {
            self.call("build", &binary.name)
        }
        fn package(&mut self, binary: &Binary) -> Result<(), AppError> {
            self.call("package", &binary.name)
        }
        fn upload(&mut self, binary: &Binary) -> Result<(), AppError> {
            self.call("upload", &binary.name)
        }
        fn cleanup(&mut self) -> Result<(), AppError> {
            self.call("cleanup", "")
        }
    }

    fn batch() -> Batch {
        Batch {
            triple: "native".into(),
            os: "runner".into(),
            timeout_minutes: timeout_minutes(2),
            binaries: vec![binary("alpha"), binary("beta")],
        }
    }

    #[test]
    fn complete_releases_require_neither_source_nor_build() {
        let mut fake = Fake {
            complete: true,
            ..Fake::default()
        };
        let outcomes = execute(&batch(), false, &mut fake).unwrap();
        assert!(outcomes.iter().all(|o| o.status == "skipped-complete"));
        assert_eq!(fake.calls, ["refresh:alpha", "refresh:beta"]);
    }

    #[test]
    fn staging_does_not_query_or_mutate_releases() {
        let mut fake = Fake::default();
        let outcomes = execute(&batch(), true, &mut fake).unwrap();
        assert!(outcomes.iter().all(|o| o.status == "staged-only"));
        assert_eq!(
            fake.calls,
            [
                "source:alpha",
                "build:alpha",
                "package:alpha",
                "build:beta",
                "package:beta",
                "cleanup:",
            ]
        );
    }

    #[test]
    fn item_failures_preserve_independent_work_and_cleanup() {
        for stage in ["refresh", "build", "package", "upload"] {
            let mut fake = Fake {
                fail: format!("{stage}:alpha"),
                ..Fake::default()
            };
            let outcomes = execute(&batch(), false, &mut fake).unwrap();
            assert!(
                outcomes
                    .iter()
                    .any(|o| o.binary.name == "alpha" && o.status == "failed" && o.stage == stage)
            );
            assert!(
                outcomes
                    .iter()
                    .any(|o| o.binary.name == "beta" && o.status == "published")
            );
            assert_eq!(fake.calls.last().unwrap(), "cleanup:");
        }
    }

    #[test]
    fn failed_source_only_blocks_its_own_group() {
        let mut batch = batch();
        batch.binaries[1].source_sha = "b".repeat(40);
        let mut fake = Fake {
            fail: "source:alpha".into(),
            ..Fake::default()
        };
        let outcomes = execute(&batch, false, &mut fake).unwrap();
        assert_eq!(outcomes[0].status, "failed");
        assert_eq!(outcomes[1].status, "published");
        assert!(!fake.calls.contains(&"build:alpha".into()));
        assert_eq!(fake.calls.iter().filter(|c| *c == "cleanup:").count(), 2);
    }

    #[test]
    fn cleanup_errors_are_retained_without_hiding_item_errors() {
        let mut fake = Fake {
            fail: "cleanup:".into(),
            ..Fake::default()
        };
        let outcomes = execute(&batch(), false, &mut fake).unwrap();
        assert_eq!(
            outcomes.iter().filter(|o| o.status == "published").count(),
            2
        );
        assert_eq!(outcomes.len(), 2);
        assert!(outcomes.iter().all(|o| o.cleanup_error.is_some()));
    }

    #[test]
    fn cancellation_prevents_later_uploads_and_preserves_cleanup() {
        let mut fake = Fake {
            cancel_after: "build:alpha".into(),
            ..Fake::default()
        };
        let outcomes = execute(&batch(), false, &mut fake).unwrap();
        assert_eq!(outcomes[0].stage, "cancelled");
        assert_eq!(outcomes[1].status, "unattempted");
        assert!(!fake.calls.iter().any(|call| call.starts_with("upload:")));
        assert_eq!(fake.calls.last().unwrap(), "cleanup:");
    }
}
