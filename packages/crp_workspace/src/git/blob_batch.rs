//! Length-framed historical blob acquisition without per-manifest Git processes.

use std::path::Path;
use std::str;

use ohno::AppError;

use crate::command::run_capture_input_bytes;

// Native forwarding only; acquire owns request admission and response interpretation.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn read(ids: &[&str], root: &Path) -> Result<Vec<Vec<u8>>, AppError> {
    acquire(ids, |input| {
        run_capture_input_bytes("git", &["cat-file", "--batch"], input, root).map_err(Into::into)
    })
}

fn acquire(
    ids: &[&str],
    run: impl FnOnce(&[u8]) -> Result<Vec<u8>, AppError>,
) -> Result<Vec<Vec<u8>>, AppError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut input = Vec::new();
    for id in ids {
        // Requests use object identities from ls-tree, never line-delimited file names.
        // This also prevents an accidental revision/path expression from changing framing.
        if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(InvalidBlobBatch::new().into());
        }
        input.extend_from_slice(id.as_bytes());
        input.push(b'\n');
    }
    let output = run(&input)?;
    decode_blob_batch(ids, &output)
}

/// Decodes an ordered batch of recorded blob identities and length-framed bytes.
pub fn decode_blob_batch(ids: &[&str], mut output: &[u8]) -> Result<Vec<Vec<u8>>, AppError> {
    let mut blobs = Vec::with_capacity(ids.len());
    for id in ids {
        let end = output
            .iter()
            .position(|byte| *byte == b'\n')
            .ok_or_else(InvalidBlobBatch::new)?;
        let (header, rest) = output
            .split_at_checked(end)
            .ok_or_else(InvalidBlobBatch::new)?;
        let header = str::from_utf8(header).map_err(InvalidBlobBatch::caused_by)?;
        let mut fields = header.split(' ');
        if fields.next() != Some(*id) || fields.next() != Some("blob") {
            return Err(InvalidBlobBatch::new().into());
        }
        let size = fields
            .next()
            .ok_or_else(InvalidBlobBatch::new)?
            .parse::<usize>()
            .map_err(InvalidBlobBatch::caused_by)?;
        if fields.next().is_some() {
            return Err(InvalidBlobBatch::new().into());
        }
        output = rest.strip_prefix(b"\n").ok_or_else(InvalidBlobBatch::new)?;
        let (blob, rest) = output
            .split_at_checked(size)
            .ok_or_else(InvalidBlobBatch::new)?;
        output = rest.strip_prefix(b"\n").ok_or_else(InvalidBlobBatch::new)?;
        blobs.push(blob.to_vec());
    }
    if !output.is_empty() {
        return Err(InvalidBlobBatch::new().into());
    }
    Ok(blobs)
}

/// A recorded blob is unavailable or the Git batch response is inconsistent with its request.
#[ohno::error]
#[display("Git could not return the requested historical blobs as a complete batch")]
struct InvalidBlobBatch;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::io;

    use super::*;

    #[test]
    fn batch_preserves_order_duplicates_empty_blobs_and_binary_framing() {
        let result = acquire(&["ab", "cd", "ab"], |input| {
            assert_eq!(input, b"ab\ncd\nab\n");
            Ok(b"ab blob 4\n\0\n\xffx\ncd blob 0\n\nab blob 4\n\0\n\xffx\n".to_vec())
        })
        .unwrap();
        assert_eq!(
            result,
            [b"\0\n\xffx".to_vec(), vec![], b"\0\n\xffx".to_vec()]
        );
        assert!(acquire(&[], |_| panic!("no objects")).unwrap().is_empty());
    }

    #[test]
    fn batch_rejects_non_identity_requests_before_execution() {
        for id in ["", "HEAD", "ab\ncd", "ab:path", "--help"] {
            let error = acquire(&[id], |_| panic!("invalid request")).unwrap_err();
            assert!(error.find_source::<InvalidBlobBatch>().is_some());
        }
    }

    #[test]
    fn batch_rejects_missing_wrong_truncated_and_extra_responses() {
        for output in [
            &b""[..],
            b"ab missing\n",
            b"cd blob 0\n\n",
            b"ab tree 0\n\n",
            b"ab blob\n",
            b"ab blob -1\n",
            b"ab blob 999999999999999999999999999\n",
            b"ab blob 0 extra\n\n",
            b"ab blob 2\nx\n",
            b"ab blob 1\nx",
            b"ab blob 1\nx!",
            b"ab blob 0\n\nextra",
            b"ab blob \xff\n",
        ] {
            let error = decode_blob_batch(&["ab"], output).unwrap_err();
            assert!(error.find_source::<InvalidBlobBatch>().is_some());
        }
        decode_blob_batch(&["ab", "cd"], b"ab blob 0\n\n").unwrap_err();
        decode_blob_batch(&[], b"extra").unwrap_err();
    }

    #[test]
    fn batch_preserves_execution_failures() {
        let error = acquire(&["ab"], |_| Err(io::Error::other("batch").into())).unwrap_err();
        assert!(error.find_source::<io::Error>().is_some());
    }
}
