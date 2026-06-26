//! The on-the-wire byte format for stored objects: gzip.
//!
//! Stored objects are repetitive JSON (per-object ratios of ~10–30×), and the
//! Azure backend transfers every stored byte on upload and on every `analyze`
//! read, so the storage layer compresses object bodies transparently. This module
//! is the single source of that encoding: the `cargo-bench-history` storage
//! backends call it on `put`/`get`, and the stress harness calls the *same*
//! function when it writes its synthetic tree, so the harness can never drift out
//! of lockstep with production.
//!
//! gzip (via the pure-Rust `miniz_oxide` backend) is chosen for three properties
//! this layer depends on:
//!
//! * **Determinism** — the same input always produces byte-identical output (the
//!   gzip header's mtime is zero and the OS byte is fixed), so a stress seed is
//!   reproducible and an object's bytes are a pure function of its contents.
//! * **Self-describing framing** — the gzip magic (`0x1f 0x8b`) never collides
//!   with a JSON object's first byte (`{` / whitespace), so [`decompress`] returns
//!   a clean error on a legacy uncompressed object rather than silent garbage.
//! * **Standard `Content-Encoding`** — `gzip` is a registered HTTP content
//!   coding, so the Azure backend can declare it and any non-SDK reader can
//!   inflate the blob with off-the-shelf tooling.

use std::io::{Read as _, Write as _};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;

/// The deflate level applied to every stored object.
///
/// Level 6 (`flate2`'s default) balances ratio against CPU. This data is written
/// once and read many times, so a higher level (up to 9) would be a defensible
/// tune if read-side wire volume ever dominated; level 6 already removes the bulk
/// of the redundancy at a fraction of level 9's compression cost.
const GZIP_LEVEL: u32 = 6;

/// gzip-compresses `plain` for storage.
///
/// Infallible: the encoder writes into an in-memory [`Vec`], and writing to a
/// `Vec` never fails, so there is no I/O error to surface (an allocation failure
/// aborts rather than returning).
#[must_use]
pub fn compress(plain: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::new(GZIP_LEVEL));
    encoder
        .write_all(plain)
        .expect("writing to an in-memory Vec cannot fail");
    encoder
        .finish()
        .expect("flushing gzip into an in-memory Vec cannot fail")
}

/// Inflates a gzip-compressed object body produced by [`compress`].
///
/// # Errors
///
/// Returns an [`std::io::Error`] if `stored` is not a valid gzip stream — a
/// corrupt or truncated body, or a legacy uncompressed object whose first bytes
/// are not the gzip magic. Decoding never silently returns partial or wrong data.
pub fn decompress(stored: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut decoder = GzDecoder::new(stored);
    let mut plain = Vec::new();
    decoder.read_to_end(&mut plain)?;
    Ok(plain)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    /// A representative repetitive object body — the kind of JSON the storage
    /// layer actually stores, where the same field names recur on every record.
    fn sample_json() -> Vec<u8> {
        let record = r#"{"id":["fast_time","capture","two_instants"],"metrics":[{"kind":"WallTime","value":12.5}]},"#;
        let mut body = String::from(r#"{"schema":1,"results":["#);
        for _ in 0..64 {
            body.push_str(record);
        }
        body.push_str("]}");
        body.into_bytes()
    }

    #[test]
    fn roundtrips_representative_json() {
        let plain = sample_json();
        let restored = decompress(&compress(&plain)).unwrap();
        assert_eq!(restored, plain);
    }

    #[test]
    fn roundtrips_empty_input() {
        let restored = decompress(&compress(b"")).unwrap();
        assert_eq!(restored, b"");
    }

    #[test]
    fn roundtrips_non_utf8_bytes() {
        // The codec is byte-oriented; it must not assume its input is text.
        let plain: Vec<u8> = (0_u8..=255).cycle().take(1000).collect();
        let restored = decompress(&compress(&plain)).unwrap();
        assert_eq!(restored, plain);
    }

    #[test]
    fn is_deterministic() {
        let plain = sample_json();
        // Byte-identical output across calls is what makes a stress seed
        // reproducible; pin against re-compression rather than a brittle golden.
        assert_eq!(compress(&plain), compress(&plain));
    }

    #[test]
    fn emits_gzip_magic() {
        let compressed = compress(&sample_json());
        assert!(
            compressed.starts_with(&[0x1f, 0x8b]),
            "a stored body must carry the gzip magic so legacy plaintext is distinguishable"
        );
    }

    #[test]
    fn compresses_repetitive_input() {
        let plain = sample_json();
        let compressed = compress(&plain);
        assert!(
            compressed.len() < plain.len(),
            "repetitive JSON must shrink: {} -> {}",
            plain.len(),
            compressed.len()
        );
    }

    #[test]
    fn decompress_rejects_plaintext_json() {
        // A legacy uncompressed object: its first byte is `{`, never the gzip
        // magic, so decoding fails loudly instead of returning the raw bytes.
        let error = decompress(br#"{"schema":1}"#).unwrap_err();
        drop(error);
    }

    #[test]
    fn decompress_rejects_truncated_stream() {
        let mut compressed = compress(&sample_json());
        compressed.truncate(compressed.len().div_ceil(2));
        let error = decompress(&compressed).unwrap_err();
        drop(error);
    }

    #[test]
    fn decompress_rejects_empty_input() {
        let error = decompress(b"").unwrap_err();
        drop(error);
    }
}
