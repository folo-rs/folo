// Immutable object acquisition for historical snapshots and released-content reads.
//
// One finite cat-file batch replaces individual object-reading subprocesses.
// Framing is checked eagerly; interpreting a blob as a manifest belongs to its
// consumer, so excluded or otherwise unused manifests remain uninterpreted bytes.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::Path;
use std::str;

use ohno::AppError;

use crate::command::run_capture_input_bytes;
use crate::text::Quotable as _;

/// Captured Git objects whose contents are interpreted only when requested.
///
/// Missing objects and non-blob types remain explicit entries after acquisition.
/// Payload ranges borrow from one owned batch buffer rather than copying every
/// object. Manifest UTF-8 and TOML validation remains the consumer's responsibility.
#[derive(Clone, Debug, Default)]
pub(crate) struct GitObjects {
    output: Vec<u8>,
    objects: HashMap<String, GitObject>,
}

impl GitObjects {
    /// Reads unique full lowercase object IDs, never paths or ref expressions.
    pub(crate) fn read(root: &Path, ids: &[&str]) -> Result<Self, AppError> {
        validate_ids(ids)?;
        let mut input = Vec::new();
        for &id in ids {
            input.extend_from_slice(id.as_bytes());
            input.push(b'\n');
        }
        if ids.is_empty() {
            return Ok(Self::default());
        }
        let output = run_capture_input_bytes("git", &["cat-file", "--batch"], &input, root)?;
        Self::parse(ids, output)
    }

    /// Returns raw blob bytes, rejecting missing, unrequested, or non-blob objects.
    pub(crate) fn blob(&self, id: &str) -> Result<&[u8], AppError> {
        match self.objects.get(id) {
            None => Err(UnrequestedGitObjectError::new(id).into()),
            Some(GitObject::Missing) => Err(MissingGitObjectError::new(id).into()),
            Some(GitObject::Present { kind, contents }) => {
                if *kind != GitObjectKind::Blob {
                    return Err(NonBlobGitObjectError::new(id, kind.as_str()).into());
                }
                Ok(self
                    .output
                    .get(contents.clone())
                    .expect("parsing validates every payload range against the owned output"))
            }
        }
    }

    fn parse(ids: &[&str], output: Vec<u8>) -> Result<Self, AppError> {
        validate_ids(ids)?;
        let mut objects = HashMap::new();
        let mut remaining = output.as_slice();
        for &id in ids {
            let header_end = remaining
                .iter()
                .position(|&byte| byte == b'\n')
                .ok_or_else(|| GitObjectProtocolError::new("missing or unterminated header"))?;
            let (header, after_header) = remaining.split_at(header_end);
            remaining = after_header
                .strip_prefix(b"\n")
                .expect("header_end locates a newline in this same slice");
            let header = str::from_utf8(header).map_err(|error| {
                GitObjectProtocolError::caused_by("object header is not UTF-8", error)
            })?;
            let mut fields = header.split(' ');
            if fields.next() != Some(id) {
                return Err(
                    GitObjectProtocolError::new("unexpected object ID or response order").into(),
                );
            }
            let entry = match (fields.next(), fields.next(), fields.next()) {
                (Some("missing"), None, None) => GitObject::Missing,
                (Some(kind), Some(size), None) => {
                    let kind = GitObjectKind::parse(kind)?;
                    if size.is_empty() || !size.bytes().all(|byte| byte.is_ascii_digit()) {
                        return Err(GitObjectProtocolError::new("invalid object size").into());
                    }
                    let size = size.parse::<usize>().map_err(|error| {
                        GitObjectProtocolError::caused_by(
                            "object size exceeds addressable memory",
                            error,
                        )
                    })?;
                    let (_, after_payload) = remaining
                        .split_at_checked(size)
                        .ok_or_else(|| GitObjectProtocolError::new("truncated object payload"))?;
                    let start = output
                        .len()
                        .checked_sub(remaining.len())
                        .expect("remaining is a suffix of the captured output");
                    let end = output
                        .len()
                        .checked_sub(after_payload.len())
                        .expect("after_payload is a suffix of the captured output");
                    remaining = after_payload
                        .strip_prefix(b"\n")
                        .ok_or_else(|| GitObjectProtocolError::new("missing object separator"))?;
                    GitObject::Present {
                        kind,
                        contents: start..end,
                    }
                }
                _ => return Err(GitObjectProtocolError::new("unexpected header fields").into()),
            };
            objects.insert(id.to_owned(), entry);
        }
        if !remaining.is_empty() {
            return Err(GitObjectProtocolError::new("unexpected trailing batch output").into());
        }
        Ok(Self { output, objects })
    }
}

/// One framed batch response, including Git's successful missing-object response.
#[derive(Clone, Debug)]
enum GitObject {
    Missing,
    Present {
        kind: GitObjectKind,
        contents: Range<usize>,
    },
}

/// The object kinds supported by Git's raw object protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GitObjectKind {
    Blob,
    Tree,
    Commit,
    Tag,
}

impl GitObjectKind {
    fn parse(kind: &str) -> Result<Self, AppError> {
        match kind {
            "blob" => Ok(Self::Blob),
            "tree" => Ok(Self::Tree),
            "commit" => Ok(Self::Commit),
            "tag" => Ok(Self::Tag),
            _ => Err(GitObjectProtocolError::new("unsupported object type").into()),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Blob => "blob",
            Self::Tree => "tree",
            Self::Commit => "commit",
            Self::Tag => "tag",
        }
    }
}

fn validate_ids(ids: &[&str]) -> Result<(), AppError> {
    // Git supports these hexadecimal widths for SHA-1 and SHA-256 object formats.
    const SHA1_HEX_LENGTH: usize = 40;
    const SHA256_HEX_LENGTH: usize = 64;

    let mut requested = HashSet::new();
    for &id in ids {
        if !matches!(id.len(), SHA1_HEX_LENGTH | SHA256_HEX_LENGTH)
            || !id
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(InvalidGitObjectIdError::new(id).into());
        }
        if !requested.insert(id) {
            return Err(DuplicateGitObjectIdError::new(id).into());
        }
    }
    Ok(())
}

/// An object request is not a complete canonical Git object ID.
#[ohno::error]
#[display("Not a full lowercase Git object ID: '{}'", id.quoted())]
struct InvalidGitObjectIdError {
    id: String,
}

/// A batch requests the same immutable object more than once.
#[ohno::error]
#[display("Git object '{}' is requested more than once in this batch", id.quoted())]
struct DuplicateGitObjectIdError {
    id: String,
}

/// A successful Git process did not return the requested, correctly framed batch.
#[ohno::error]
#[display("Invalid git cat-file batch response: {reason}")]
struct GitObjectProtocolError {
    reason: String,
}

/// A consumer requested an ID outside this captured batch.
#[ohno::error]
#[display("Git object '{}' was not requested in this batch", id.quoted())]
struct UnrequestedGitObjectError {
    id: String,
}

/// Git reported a missing object that a consumer now requires.
#[ohno::error]
#[display("Required Git object '{}' is missing", id.quoted())]
struct MissingGitObjectError {
    id: String,
}

/// A consumer requested file contents from a captured non-blob object.
#[ohno::error]
#[display("Git object '{}' has type {kind}, not blob", id.quoted())]
struct NonBlobGitObjectError {
    id: String,
    kind: String,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    // Distinct full IDs make response ordering observable without hashing fixture bytes.
    const FIRST: &str = "0123456789abcdef0123456789abcdef01234567";
    const SECOND: &str = "89abcdef0123456789abcdef0123456789abcdef";
    const THIRD: &str = "fedcba9876543210fedcba9876543210fedcba98";
    const SHA256: &str = concat!(
        "0123456789abcdef",
        "0123456789abcdef",
        "0123456789abcdef",
        "0123456789abcdef"
    );

    fn response(id: &str, kind: &str, payload: &[u8]) -> Vec<u8> {
        let mut output = format!("{id} {kind} {}\n", payload.len()).into_bytes();
        output.extend_from_slice(payload);
        output.push(b'\n');
        output
    }

    fn assert_protocol_error(ids: &[&str], output: Vec<u8>) {
        let error = GitObjects::parse(ids, output).unwrap_err();
        assert!(error.find_source::<GitObjectProtocolError>().is_some());
    }

    #[test]
    fn binary_and_empty_objects_preserve_payload_boundaries() {
        let binary = b"\0\xfffirst\n\nlast\r\n\0";
        let mut output = response(FIRST, "blob", binary);
        output.extend(response(SECOND, "blob", b""));
        output.extend(response(SHA256, "blob", b"after an empty object"));
        let objects = GitObjects::parse(&[FIRST, SECOND, SHA256], output).unwrap();

        assert_eq!(objects.blob(FIRST).unwrap(), binary);
        assert_eq!(objects.blob(SECOND).unwrap(), b"");
        assert_eq!(objects.blob(SHA256).unwrap(), b"after an empty object");
    }

    #[test]
    fn cloned_objects_preserve_payloads_and_deferred_errors_after_the_original_is_dropped() {
        let binary = b"\0\xffcloned\n";
        let cloned = {
            let mut output = response(FIRST, "blob", binary);
            output.extend_from_slice(format!("{SECOND} missing\n").as_bytes());
            output.extend(response(THIRD, "tree", b"non-blob"));
            let objects = GitObjects::parse(&[FIRST, SECOND, THIRD], output).unwrap();
            let cloned = objects.clone();
            assert_eq!(objects.blob(FIRST).unwrap(), cloned.blob(FIRST).unwrap());
            cloned
        };

        assert_eq!(cloned.blob(FIRST).unwrap(), binary);
        let error = cloned.blob(SECOND).unwrap_err();
        assert!(error.find_source::<MissingGitObjectError>().is_some());
        let error = cloned.blob(THIRD).unwrap_err();
        assert!(error.find_source::<NonBlobGitObjectError>().is_some());
    }

    #[test]
    fn payloads_that_resemble_protocol_headers_are_not_parsed_as_headers() {
        let payload = format!("\n{SECOND} missing\n{THIRD} blob 0\n\n");
        let objects =
            GitObjects::parse(&[FIRST], response(FIRST, "blob", payload.as_bytes())).unwrap();
        assert_eq!(objects.blob(FIRST).unwrap(), payload.as_bytes());
    }

    #[test]
    fn missing_objects_are_retained_without_blocking_other_objects() {
        let mut output = format!("{FIRST} missing\n").into_bytes();
        output.extend(response(SECOND, "blob", b"needed"));
        output.extend_from_slice(format!("{THIRD} missing\n").as_bytes());
        let objects = GitObjects::parse(&[FIRST, SECOND, THIRD], output).unwrap();

        assert!(matches!(
            objects.objects.get(FIRST),
            Some(GitObject::Missing)
        ));
        assert_eq!(objects.blob(SECOND).unwrap(), b"needed");
        for id in [FIRST, THIRD] {
            let error = objects.blob(id).unwrap_err();
            assert!(error.find_source::<MissingGitObjectError>().is_some());
        }
    }

    #[test]
    fn manifest_content_is_not_decoded_or_parsed_during_acquisition() {
        let mut output = response(FIRST, "blob", b"\xff\xfe\0");
        output.extend(response(SECOND, "blob", b"[package = = not toml"));
        output.extend(response(THIRD, "blob", b"needed"));
        let objects = GitObjects::parse(&[FIRST, SECOND, THIRD], output).unwrap();

        assert_eq!(objects.blob(THIRD).unwrap(), b"needed");
        assert_eq!(objects.blob(FIRST).unwrap(), b"\xff\xfe\0");
        assert_eq!(objects.blob(SECOND).unwrap(), b"[package = = not toml");
    }

    #[test]
    fn non_blob_types_are_rejected_only_when_used_as_file_contents() {
        for (kind, expected) in [
            ("tree", GitObjectKind::Tree),
            ("commit", GitObjectKind::Commit),
            ("tag", GitObjectKind::Tag),
        ] {
            let mut output = response(FIRST, kind, b"\0arbitrary object contents");
            output.extend(response(SECOND, "blob", b"needed"));
            let objects = GitObjects::parse(&[FIRST, SECOND], output).unwrap();

            assert!(matches!(
                objects.objects.get(FIRST),
                Some(GitObject::Present { kind, .. }) if *kind == expected
            ));
            assert_eq!(objects.blob(SECOND).unwrap(), b"needed");
            let error = objects.blob(FIRST).unwrap_err();
            let error = error.find_source::<NonBlobGitObjectError>().unwrap();
            assert_eq!(error.kind, kind);
        }
    }

    #[test]
    fn unrequested_and_reported_missing_objects_are_distinct() {
        let objects =
            GitObjects::parse(&[FIRST], format!("{FIRST} missing\n").into_bytes()).unwrap();
        let error = objects.blob(SECOND).unwrap_err();
        assert!(error.find_source::<UnrequestedGitObjectError>().is_some());
    }

    #[test]
    fn duplicate_requests_are_rejected_before_launching_git() {
        let ids = [FIRST, FIRST];
        let error = GitObjects::read(Path::new("not-a-repository"), &ids).unwrap_err();
        assert!(error.find_source::<DuplicateGitObjectIdError>().is_some());
        let error = GitObjects::parse(&ids, response(FIRST, "blob", b"same object")).unwrap_err();
        assert!(error.find_source::<DuplicateGitObjectIdError>().is_some());
    }

    #[test]
    fn empty_requests_need_no_repository_or_subprocess() {
        for objects in [
            GitObjects::default(),
            GitObjects::read(Path::new("not-a-repository"), &[]).unwrap(),
            GitObjects::parse(&[], Vec::new()).unwrap(),
        ] {
            assert!(objects.objects.is_empty());
            assert!(objects.output.is_empty());
            let error = objects.blob(FIRST).unwrap_err();
            assert!(error.find_source::<UnrequestedGitObjectError>().is_some());
        }
        assert_protocol_error(&[], response(FIRST, "blob", b"unexpected"));
    }

    #[test]
    fn requests_reject_refs_paths_abbreviations_and_line_injection() {
        for id in [
            "",
            "HEAD",
            "HEAD:Cargo.toml",
            ":Cargo.toml",
            "0123456",
            "0123456789abcdef0123456789abcdef012345678",
            "0123456789abcdef0123456789abcdef0123456g",
            "ABCDEF0123456789abcdef0123456789abcdef01",
            "0123456789abcdef0123456789abcdef0123456\n",
            "0123456789abcdef0123456789abcdef0123456\0",
        ] {
            let error = GitObjects::read(Path::new("not-a-repository"), &[id]).unwrap_err();
            assert!(error.find_source::<InvalidGitObjectIdError>().is_some());
        }
    }

    #[test]
    fn headers_must_name_the_expected_object_in_request_order() {
        assert_protocol_error(&[FIRST], response(SECOND, "blob", b"wrong ID"));
        assert_protocol_error(&[FIRST], format!("{SECOND} missing\n").into_bytes());
        let mut output = response(SECOND, "blob", b"second");
        output.extend(response(FIRST, "blob", b"first"));
        assert_protocol_error(&[FIRST, SECOND], output);
    }

    #[test]
    fn headers_require_exact_fields_and_supported_types() {
        for suffix in [
            "",
            " blob",
            " blob 0 extra",
            "  blob 0",
            "\tblob 0",
            " blob 0 ",
            " missing extra",
            " ambiguous",
            " nonsense 0",
            " blob 0\r",
        ] {
            assert_protocol_error(&[FIRST], format!("{FIRST}{suffix}\n\n").into_bytes());
        }
        let mut output = format!("{FIRST} ").into_bytes();
        output.extend_from_slice(b"\xff 0\n\n");
        assert_protocol_error(&[FIRST], output);
    }

    #[test]
    fn sizes_must_be_unsigned_decimal_values_that_fit_memory() {
        for size in ["", "-1", "+1", "1.0", "0x1", "184467440737095516160"] {
            assert_protocol_error(&[FIRST], format!("{FIRST} blob {size}\nx\n").into_bytes());
        }
        assert_protocol_error(
            &[FIRST],
            format!("{FIRST} blob {}\nx\n", usize::MAX).into_bytes(),
        );
    }

    #[test]
    fn truncated_headers_payloads_and_separators_are_rejected() {
        let output = response(FIRST, "blob", b"\0\n\xff");
        for end in 0..output.len() {
            assert_protocol_error(&[FIRST], output.get(..end).unwrap().to_vec());
        }
        assert_protocol_error(&[FIRST], format!("{FIRST} missing").into_bytes());
        assert_protocol_error(&[FIRST], format!("{FIRST} blob 0\n").into_bytes());
    }

    #[test]
    fn payload_sizes_and_separators_must_match_the_wire_bytes() {
        for tail in [b"abX\n".as_slice(), b"abc\n", b"a\n"] {
            let mut output = format!("{FIRST} blob 2\n").into_bytes();
            output.extend_from_slice(tail);
            assert_protocol_error(&[FIRST], output);
        }
        assert_protocol_error(&[FIRST], format!("{FIRST} blob 0\n\r\n").into_bytes());
    }

    #[test]
    fn missing_responses_extra_responses_and_trailing_data_are_rejected() {
        assert_protocol_error(&[FIRST, SECOND], response(FIRST, "blob", b"first"));
        for trailing in [
            b"\n".to_vec(),
            b"\0garbage".to_vec(),
            response(FIRST, "blob", b"duplicate response"),
            response(SECOND, "blob", b"extra"),
            format!("{SECOND} missing\n").into_bytes(),
        ] {
            let mut output = response(FIRST, "blob", b"first");
            output.extend(trailing);
            assert_protocol_error(&[FIRST], output);
        }
    }
}
