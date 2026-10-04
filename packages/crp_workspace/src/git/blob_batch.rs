//! Length-framed object acquisition and size queries without per-file Git processes.

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

// Native forwarding only; size acquisition uses the same admitted identity protocol.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn sizes(ids: &[&str], root: &Path) -> Result<Vec<usize>, AppError> {
    let input = requests(ids)?;
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let output = run_capture_input_bytes("git", &["cat-file", "--batch-check"], &input, root)?;
    decode_blob_sizes(ids, &output)
}

fn acquire(
    ids: &[&str],
    run: impl FnOnce(&[u8]) -> Result<Vec<u8>, AppError>,
) -> Result<Vec<Vec<u8>>, AppError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let input = requests(ids)?;
    let output = run(&input)?;
    decode_blob_batch(ids, &output)
}

fn requests(ids: &[&str]) -> Result<Vec<u8>, AppError> {
    let mut input = Vec::new();
    for id in ids {
        // Requests use object identities, never line-delimited file names.
        // This also prevents an accidental revision/path expression from changing framing.
        if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(InvalidBlobBatch::new().into());
        }
        input.extend_from_slice(id.as_bytes());
        input.push(b'\n');
    }
    Ok(input)
}

/// Interprets ordered blob sizes using the content reader's header admission.
pub fn decode_blob_sizes(ids: &[&str], mut output: &[u8]) -> Result<Vec<usize>, AppError> {
    let mut sizes = Vec::with_capacity(ids.len());
    for id in ids {
        sizes.push(header(id, &mut output)?);
    }
    if !output.is_empty() {
        return Err(InvalidBlobBatch::new().into());
    }
    Ok(sizes)
}

fn header(id: &str, output: &mut &[u8]) -> Result<usize, AppError> {
    let end = output
        .iter()
        .position(|byte| *byte == b'\n')
        .ok_or_else(InvalidBlobBatch::new)?;
    let (header, rest) = output
        .split_at_checked(end)
        .ok_or_else(InvalidBlobBatch::new)?;
    let header = str::from_utf8(header).map_err(InvalidBlobBatch::caused_by)?;
    let mut fields = header.split(' ');
    if fields.next() != Some(id) || fields.next() != Some("blob") {
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
    *output = rest.strip_prefix(b"\n").ok_or_else(InvalidBlobBatch::new)?;
    Ok(size)
}

/// Decodes an ordered batch of recorded blob identities and length-framed bytes.
pub fn decode_blob_batch(ids: &[&str], mut output: &[u8]) -> Result<Vec<Vec<u8>>, AppError> {
    let mut blobs = Vec::with_capacity(ids.len());
    for id in ids {
        let size = header(id, &mut output)?;
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
#[display("Git could not return the requested blobs as a complete batch")]
pub(crate) struct InvalidBlobBatch;

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

    #[test]
    fn sizes_preserve_order_and_require_complete_blob_headers() {
        assert_eq!(
            decode_blob_sizes(&["ab", "cd", "ab"], b"ab blob 4\ncd blob 0\nab blob 4\n").unwrap(),
            [4, 0, 4]
        );
        assert!(decode_blob_sizes(&[], b"").unwrap().is_empty());
        for output in [
            &b""[..],
            b"ab missing\n",
            b"cd blob 4\n",
            b"ab tree 4\n",
            b"ab blob 4",
            b"ab blob -1\n",
            b"ab blob 4 extra\n",
            b"ab blob 4\nextra",
        ] {
            assert!(
                decode_blob_sizes(&["ab"], output)
                    .unwrap_err()
                    .find_source::<InvalidBlobBatch>()
                    .is_some()
            );
        }
    }
}
