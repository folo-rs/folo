//! Historical installation acquisition and independent lockfile endpoints.

use super::*;

#[test]
fn lockfile_decoding_preserves_text_and_rejects_non_utf8() {
    let text = "[[package]]\nname = 'tool'\nversion = '1.0.0'\n";
    assert_eq!(
        decode_lockfile(text.as_bytes().to_vec(), "Cargo.lock").unwrap(),
        text
    );
    assert_eq!(decode_lockfile(Vec::new(), "Cargo.lock").unwrap(), "");
    assert!(
        decode_lockfile(vec![0xff], "Cargo.lock")
            .unwrap_err()
            .find_source::<MalformedLockfileError>()
            .is_some()
    );
}
