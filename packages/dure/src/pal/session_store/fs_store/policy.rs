//! In-process record selection and ownership policy for the filesystem store.
//!
//! Native adapters supply bytes and I/O results; these decisions need no real files.
//! Ref: docs/session-store.md, "Filesystem policy and native operations".

use std::ffi::OsStr;
use std::io;

use crate::SessionId;
use crate::pal::error::{PalError, PalErrorKind};
use crate::pal::session_store::stored::StoredSession;
use crate::session_record::{ProcessIdentity, SessionRecord};

/// Decodes the stored claim or publication without hiding malformed content.
pub(crate) fn parse_stored(bytes: &[u8]) -> Result<StoredSession, PalError> {
    serde_json::from_slice(bytes).map_err(|error| {
        PalError::with_source(
            PalErrorKind::Other,
            io::Error::new(io::ErrorKind::InvalidData, error),
        )
    })
}

/// Selects only the published record the caller requested.
pub(crate) fn parse_record(
    bytes: &[u8],
    expected: SessionId,
) -> Result<Option<SessionRecord>, PalError> {
    // An empty provisional file does not yet describe a session.
    if bytes.is_empty() {
        return Ok(None);
    }
    Ok(published_record(expected, parse_stored(bytes)?))
}

/// Recognizes names readers treat as session files rather than staging or foreign files.
pub(crate) fn record_id(name: &OsStr) -> Option<SessionId> {
    name.to_str()?
        .strip_suffix(".json")?
        .parse::<u32>()
        .ok()
        .and_then(SessionId::from_u32)
}

/// Applies the same key/record identity check to direct reads and directory listings.
fn published_record(id: SessionId, stored: StoredSession) -> Option<SessionRecord> {
    match stored {
        StoredSession::Published(record) if record.id == id => Some(record),
        _ => None,
    }
}

/// Makes listing order independent of native directory enumeration order.
pub(crate) fn sort_by_id(stored: &mut [(SessionId, StoredSession)]) {
    stored.sort_by_key(|(id, _stored)| id.get());
}

/// Keeps attachable records without exposing reservations or mismatched identities.
pub(crate) fn published_records(stored: Vec<(SessionId, StoredSession)>) -> Vec<SessionRecord> {
    stored
        .into_iter()
        .filter_map(|(id, stored)| published_record(id, stored))
        .collect()
}

/// Keeps claims available to garbage collection without treating publications as claims.
pub(crate) fn reservations(
    stored: Vec<(SessionId, StoredSession)>,
) -> Vec<(SessionId, ProcessIdentity)> {
    stored
        .into_iter()
        .filter_map(|(id, stored)| match stored {
            StoredSession::Reserved { owner } => Some((id, owner)),
            StoredSession::Published(_record) => None,
        })
        .collect()
}

/// Performs deletion only when the inspected bytes still name the requested owner.
///
/// The adapter supplies deletion through the same handle it used to read these bytes,
/// keeping replacement files outside this decision. Malformed bytes name no owner.
pub(crate) fn delete_if_owned(
    bytes: &[u8],
    owner: &ProcessIdentity,
    delete: impl FnOnce() -> io::Result<()>,
) -> Result<(), PalError> {
    let owned = match parse_stored(bytes) {
        Ok(StoredSession::Reserved { owner: current }) => current == *owner,
        Ok(StoredSession::Published(record)) => record.supervisor == *owner,
        Err(_error) => false,
    };
    if !owned {
        return Ok(());
    }
    delete().map_err(PalError::from_io)
}

/// Distinguishes an absent file or directory from a failure to access it.
pub(crate) fn if_present<T>(result: io::Result<T>) -> io::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::error::Error;
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use std::path::PathBuf;

    use super::*;
    use crate::AppCommand;
    use crate::protocol::PROTOCOL_VERSION;

    /// An arbitrary published session whose fields survive byte conversion.
    fn record() -> SessionRecord {
        SessionRecord {
            id: SessionId::MIN,
            supervisor: ProcessIdentity {
                pid: 42_u32.try_into().unwrap(),
                creation_time: 99,
            },
            pipe_name: "session-pipe".to_owned(),
            launch_directory: PathBuf::from(r"C:\work"),
            command: AppCommand::for_test(&["app.exe"]),
            started_at_unix_ms: 1,
            attached: false,
            protocol_version: PROTOCOL_VERSION,
        }
    }

    #[test]
    fn direct_reads_preserve_the_requested_record_and_reject_other_ids() {
        let record = record();
        let bytes = serde_json::to_vec(&StoredSession::Published(record.clone())).unwrap();
        assert_eq!(parse_record(&bytes, record.id).unwrap(), Some(record));
        let other_id = SessionId::from_u32(2).unwrap();
        assert!(parse_record(&bytes, other_id).unwrap().is_none());
    }

    #[test]
    fn direct_reads_distinguish_provisional_and_malformed_content() {
        let id = SessionId::MIN;
        let bytes = serde_json::to_vec(&StoredSession::Reserved {
            owner: record().supervisor,
        })
        .unwrap();
        assert!(parse_record(&bytes, id).unwrap().is_none());
        assert!(parse_record(b"", id).unwrap().is_none());
        let error = parse_record(b"not json", id).unwrap_err();
        assert_eq!(error.kind(), PalErrorKind::Other);
        assert_eq!(
            error
                .source()
                .unwrap()
                .downcast_ref::<io::Error>()
                .unwrap()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn only_session_filenames_yield_ids() {
        assert_eq!(record_id(OsStr::new("1.json")), Some(SessionId::MIN));
        for name in [
            "0.json",
            "4294967296.json",
            "other.json",
            "1.claim",
            "1.json.tmp",
        ] {
            assert!(record_id(OsStr::new(name)).is_none());
        }
        // An unpaired UTF-16 surrogate cannot be a textual session id.
        let name = OsString::from_wide(&[0xD800]);
        assert!(record_id(&name).is_none());
    }

    #[test]
    fn listings_separate_claims_from_matching_publications() {
        let record = record();
        let claim_id = SessionId::from_u32(2).unwrap();
        let owner = ProcessIdentity::for_test(7);
        let stored = vec![
            (record.id, StoredSession::Published(record.clone())),
            (claim_id, StoredSession::Reserved { owner }),
            // A copied record must not appear under another filename.
            (claim_id, StoredSession::Published(record.clone())),
        ];
        assert_eq!(published_records(stored.clone()), vec![record]);
        assert_eq!(reservations(stored), vec![(claim_id, owner)]);
        assert!(published_records(Vec::new()).is_empty());
        assert!(reservations(Vec::new()).is_empty());
    }

    #[test]
    fn listing_order_preserves_content_and_uses_ascending_ids() {
        let first = (SessionId::MIN, StoredSession::Published(record()));
        let second = (
            SessionId::from_u32(2).unwrap(),
            StoredSession::Reserved {
                owner: ProcessIdentity::for_test(7),
            },
        );
        let mut stored = vec![second.clone(), first.clone()];
        sort_by_id(&mut stored);
        assert_eq!(stored, vec![first, second]);
    }

    #[test]
    fn deletion_requires_both_halves_of_the_current_owner_identity() {
        let record = record();
        let owner = record.supervisor;
        let different_pid = ProcessIdentity {
            pid: 43_u32.try_into().unwrap(),
            ..owner
        };
        let different_creation_time = ProcessIdentity {
            creation_time: owner.creation_time + 1,
            ..owner
        };
        for stored in [
            StoredSession::Reserved { owner },
            StoredSession::Published(record),
        ] {
            let bytes = serde_json::to_vec(&stored).unwrap();
            for candidate in [different_pid, different_creation_time] {
                delete_if_owned(&bytes, &candidate, || panic!("another owner's file")).unwrap();
            }
            let mut deleted = false;
            delete_if_owned(&bytes, &owner, || {
                deleted = true;
                Ok(())
            })
            .unwrap();
            assert!(deleted);
        }
    }

    #[test]
    fn unreadable_content_does_not_authorize_deletion() {
        for bytes in [b"".as_slice(), b"not json".as_slice()] {
            delete_if_owned(bytes, &record().supervisor, || panic!("unowned file")).unwrap();
        }
    }

    #[test]
    fn an_owned_file_deletion_failure_is_reported() {
        let record = record();
        let bytes = serde_json::to_vec(&StoredSession::Published(record.clone())).unwrap();
        let error = delete_if_owned(&bytes, &record.supervisor, || {
            Err(io::ErrorKind::PermissionDenied.into())
        })
        .unwrap_err();
        assert_eq!(
            error
                .source()
                .unwrap()
                .downcast_ref::<io::Error>()
                .unwrap()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn absence_is_not_an_access_failure_or_an_empty_file() {
        // Distinct nonempty bytes also reject constant-success substitutions.
        let bytes = b"record".to_vec();
        assert_eq!(if_present(Ok(bytes.clone())).unwrap(), Some(bytes));
        assert_eq!(if_present(Ok(Vec::<u8>::new())).unwrap(), Some(Vec::new()));
        assert_eq!(
            if_present::<Vec<u8>>(Err(io::ErrorKind::NotFound.into())).unwrap(),
            None
        );
        for kind in [io::ErrorKind::PermissionDenied, io::ErrorKind::Other] {
            assert_eq!(
                if_present::<Vec<u8>>(Err(kind.into())).unwrap_err().kind(),
                kind
            );
        }
    }
}
