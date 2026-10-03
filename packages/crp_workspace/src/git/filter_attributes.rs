//! Effective filter selection, without executing a clean driver.

use std::path::Path;

use ohno::AppError;

use crate::command::run_capture_os_bytes;
use crate::git::{PATH_ARG_BUDGET, command_line_batches};

#[cfg_attr(test, mutants::skip)] // Native queries; decode preserves exact request/response identity.
pub(super) fn read(paths: &[&str], root: &Path) -> Result<bool, AppError> {
    let mut selected = false;
    for batch in command_line_batches(paths, PATH_ARG_BUDGET)? {
        let args = ["check-attr", "-z", "--all", "--"]
            .into_iter()
            .chain(batch.iter().copied());
        selected |= decode(&batch, &run_capture_os_bytes("git", args, root)?)?;
    }
    Ok(selected)
}

fn decode(paths: &[&str], mut bytes: &[u8]) -> Result<bool, AppError> {
    let mut selected = false;
    let mut paths = paths.iter();
    let mut current = None;
    while !bytes.is_empty() {
        let (path, rest) = field(bytes)?;
        if current != Some(path) {
            current = paths
                .find(|expected| expected.as_bytes() == path)
                .map(|path| path.as_bytes());
            if current.is_none() {
                return Err(InvalidFilterAttributes::new().into());
            }
        }
        let (attribute, rest) = field(rest)?;
        let (_, rest) = field(rest)?;
        if attribute.is_empty() {
            return Err(InvalidFilterAttributes::new().into());
        }
        // --all omits unspecified attributes but retains a driver literally named
        // "unspecified". Treat "unset" conservatively too: it can also be a driver name.
        selected |= attribute == b"filter";
        bytes = rest;
    }
    Ok(selected)
}

fn field(bytes: &[u8]) -> Result<(&[u8], &[u8]), AppError> {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(InvalidFilterAttributes::new)?;
    let (field, rest) = bytes.split_at(end);
    Ok((
        field,
        rest.split_first().expect("the delimiter was found").1,
    ))
}

/// Git's attribute records must refer to requested paths in order.
#[ohno::error]
#[display("Git returned an invalid filter-attribute response")]
struct InvalidFilterAttributes;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn admits_only_absence_without_interpreting_driver_names() {
        assert!(!decode(&[], b"").unwrap());
        assert!(!decode(&["a", "b"], b"a\0text\0set\0a\0eol\0lf\0").unwrap());
        for driver in [
            b"count".as_slice(),
            b"set",
            b"unset",
            b"unspecified",
            b"",
            &[0xff],
        ] {
            let mut response = b":a\nb\0filter\0".to_vec();
            response.extend(driver);
            response.push(0);
            assert!(decode(&[":a\nb"], &response).unwrap());
        }
        assert!(decode(&["a", "b"], b"b\0filter\0count\0").unwrap());
    }

    #[test]
    fn rejects_missing_reordered_malformed_and_extra_records() {
        for response in [
            b"a".as_slice(),
            b"a\0filter\0unset",
            b"a\0\0unset\0",
            b"b\0filter\0unset\0",
            b"a\0filter\0unset\0extra\0",
        ] {
            assert!(
                decode(&["a"], response)
                    .unwrap_err()
                    .find_source::<InvalidFilterAttributes>()
                    .is_some()
            );
        }
        decode(&["a", "b"], b"b\0filter\0unset\0a\0filter\0unset\0").unwrap_err();
    }
}
