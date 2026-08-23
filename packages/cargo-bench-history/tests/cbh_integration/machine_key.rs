use crate::harness::*;

/// Driving `machine-key` through the production `run` entry probes the real host
/// and returns its hardware fingerprint as the completed message. This exercises
/// the dispatch arm and the thin real-host `execute` wrapper end to end; the
/// fingerprint-derivation logic itself is unit-tested pure over a synthetic
/// profile, so here we only assert the shape CI captures and threads into
/// `--machine-key` (a single lowercase 16-hex-character line).
#[tokio::test]
#[cfg_attr(miri, ignore)] // Probes the real host, which Miri cannot do.
async fn machine_key_reports_this_host_fingerprint() {
    let outcome = run(&command_from(&["machine-key"])).await.unwrap();

    let RunOutcome::Completed { message } = outcome else {
        panic!("machine-key should complete: {outcome:?}");
    };
    assert_eq!(
        message.len(),
        16,
        "the fingerprint should be 16 hex characters: {message:?}"
    );
    assert!(
        message
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "the fingerprint should be lowercase hex: {message:?}"
    );
}
