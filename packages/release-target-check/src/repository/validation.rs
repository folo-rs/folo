//! In-memory decisions over repository evidence; Git and filesystem adapters live in snapshot.

use std::path::Path;

use ohno::AppError;

use crate::repository::VerificationError;

pub(crate) fn validate_commit(actual: &str, expected: &str) -> Result<(), AppError> {
    if actual.trim() != expected {
        return Err(VerificationError::new(format!(
            "resolved commit {} differs from requested commit {expected}",
            actual.trim()
        ))
        .into());
    }
    Ok(())
}

pub(crate) fn validate_status(status: &[u8]) -> Result<(), AppError> {
    if !status.is_empty() {
        return Err(VerificationError::new(format!(
            "candidate checkout is not clean: {}",
            String::from_utf8_lossy(status).replace('\0', "; ")
        ))
        .into());
    }
    Ok(())
}

pub(crate) fn validate_index(files: &[u8]) -> Result<(), AppError> {
    if files
        .split(|byte| *byte == b'\0')
        .filter(|entry| !entry.is_empty())
        .any(|entry| entry.first() != Some(&b'H'))
    {
        return Err(VerificationError::new(
            "candidate index contains flags that conceal tracked worktree changes",
        )
        .into());
    }
    Ok(())
}

pub(crate) fn validate_around<T>(
    mut validate: impl FnMut() -> Result<(), AppError>,
    operation: impl FnOnce() -> Result<T, AppError>,
) -> Result<T, AppError> {
    validate()?;
    let result = operation();
    // Recheck failed operations too: callers must not reuse evidence changed by a failed
    // metadata or checker invocation. The candidate is never repaired here.
    validate()?;
    result
}

pub(crate) fn validate_history(
    history: &str,
    candidate: &str,
    release_line: &str,
) -> Result<(), AppError> {
    if !history.lines().any(|commit| commit == candidate) {
        return Err(VerificationError::new(format!(
            "candidate {candidate} is not on the first-parent history of release line {release_line}"
        ))
        .into());
    }
    Ok(())
}

pub(crate) fn relative_input<'a>(path: &'a Path, root: &Path) -> Result<&'a Path, AppError> {
    path.strip_prefix(root).map_err(|error| {
        VerificationError::caused_by(
            format!(
                "release input is outside the candidate repository: {}",
                path.display()
            ),
            error,
        )
        .into()
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::RefCell;
    use std::path::StripPrefixError;

    use super::*;

    #[test]
    fn requires_the_exact_commit_identity() {
        // Distinct opaque IDs suffice: Git's object resolution is an integration concern.
        for actual in ["candidate", "candidate\n", "candidate\r\n"] {
            validate_commit(actual, "candidate").unwrap();
        }
        for actual in ["", "other\n", "candidate-extra\n"] {
            let error = validate_commit(actual, "candidate").unwrap_err();
            assert!(error.find_source::<VerificationError>().is_some());
        }
    }

    #[test]
    fn requires_empty_worktree_status() {
        validate_status(b"").unwrap();
        for status in [b" M input\0".as_slice(), b"?? extra\0", b"\xff"] {
            let error = validate_status(status).unwrap_err();
            assert!(error.find_source::<VerificationError>().is_some());
        }
    }

    #[test]
    fn requires_unconcealed_index_entries() {
        for files in [b"".as_slice(), b"H first\0", b"H first\0H second\0"] {
            validate_index(files).unwrap();
        }
        for files in [
            b"h concealed\0".as_slice(),
            b"S concealed\0",
            b"H first\0s concealed\0",
            b"H first\0? unknown\0",
        ] {
            let error = validate_index(files).unwrap_err();
            assert!(error.find_source::<VerificationError>().is_some());
        }
    }

    #[test]
    fn requires_exact_first_parent_membership() {
        for history in [
            "candidate\n",
            "tip\ncandidate\nroot\n",
            "tip\r\ncandidate\r\n",
        ] {
            validate_history(history, "candidate", "tip").unwrap();
        }
        for history in ["", "tip\nroot\n", "candidate-extra\n", "prefix-candidate\n"] {
            let error = validate_history(history, "candidate", "tip").unwrap_err();
            assert!(error.find_source::<VerificationError>().is_some());
        }
    }

    #[test]
    fn requires_repository_relative_inputs() {
        let root = Path::new("repository");
        let relative = Path::new("member").join("Cargo.toml");
        assert_eq!(
            relative_input(&root.join(&relative), root).unwrap(),
            relative
        );
        for path in [
            Path::new("external").join("Cargo.toml"),
            Path::new("repository-other").join("Cargo.toml"),
        ] {
            let error = relative_input(&path, root).unwrap_err();
            assert!(error.find_source::<VerificationError>().is_some());
            assert!(error.find_source::<StripPrefixError>().is_some());
        }
    }

    #[test]
    fn rechecks_both_operation_outcomes_and_preserves_unchanged_results() {
        for succeeds in [true, false] {
            let calls = RefCell::new(Vec::new());
            let result = validate_around(
                || {
                    calls.borrow_mut().push("validate");
                    Ok(())
                },
                || {
                    calls.borrow_mut().push("operation");
                    if succeeds {
                        Ok("operation value")
                    } else {
                        Err(OperationError::new().into())
                    }
                },
            );
            assert_eq!(*calls.borrow(), ["validate", "operation", "validate"]);
            if succeeds {
                assert_eq!(result.unwrap(), "operation value");
            } else {
                assert!(
                    result
                        .unwrap_err()
                        .find_source::<OperationError>()
                        .is_some()
                );
            }
        }
    }

    #[test]
    fn rejects_invalid_initial_evidence_without_invoking_the_operation() {
        let error = validate_around(
            || Err(VerificationError::new("invalid evidence").into()),
            || -> Result<(), AppError> { panic!("operation must not run") },
        )
        .unwrap_err();
        assert!(error.find_source::<VerificationError>().is_some());
    }

    #[test]
    fn rejects_changed_evidence_instead_of_either_operation_outcome() {
        for succeeds in [true, false] {
            let mut checks = [
                Ok(()),
                Err(VerificationError::new("changed evidence").into()),
            ]
            .into_iter();
            let error = validate_around(
                || checks.next().unwrap(),
                || {
                    if succeeds {
                        Ok(())
                    } else {
                        Err(OperationError::new().into())
                    }
                },
            )
            .unwrap_err();
            assert!(error.find_source::<VerificationError>().is_some());
            assert!(error.find_source::<OperationError>().is_none());
            assert!(checks.next().is_none());
        }
    }

    /// Distinguishes operation failure from invalid repository evidence.
    #[ohno::error]
    struct OperationError;
}
