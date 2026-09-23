//! Termination signals stop the owned build tree and still clean the prepared source.

use std::fs;
use std::io::{Read, Write as _};
use std::net::TcpListener;
use std::process::Stdio;

use serde_json::{Value, json};

use crate::{Fixture, SMOKE_WATCHDOG, run, write};

#[test]
fn cancellation_terminates_the_build_tree_and_cleans_source() {
    testing::with_watchdog_timeout(SMOKE_WATCHDOG, || {
        let mut fixture = Fixture::new();
        // The socket is an explicit readiness/termination handshake, never a time-based wait.
        // Holding it open inside the build script proves cancellation reaches Cargo descendants.
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        write(
            fixture.root.path(),
            "alpha/build.rs",
            r#"
use std::io::{Read, Write};
fn main() {
    let mut socket = std::net::TcpStream::connect(std::env::var("RELEASE_FIXTURE_SOCKET").unwrap()).unwrap();
    socket.write_all(b"ready").unwrap();
    let mut request = [0];
    socket.read_exact(&mut request).unwrap();
    socket.write_all(b"waiting").unwrap();
    socket.read_exact(&mut request).unwrap();
}
"#,
        );
        fixture.commit_source();
        let mut process = fixture
            .batch_command(
                &json!([fixture.binary("alpha"), fixture.binary("beta")]),
                "out",
            )
            .arg("--no-upload")
            .env(
                "RELEASE_FIXTURE_SOCKET",
                listener.local_addr().unwrap().to_string(),
            )
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let (mut socket, _) = listener.accept().unwrap();
        let mut ready = [0; 5];
        socket.read_exact(&mut ready).unwrap();
        assert_eq!(&ready, b"ready");
        socket.write_all(b"x").unwrap();
        let mut waiting = [0; 7];
        socket.read_exact(&mut waiting).unwrap();
        assert_eq!(&waiting, b"waiting");
        run(
            fixture.root.path(),
            "kill",
            &["-TERM", &process.id().to_string()],
        );
        assert!(!process.wait().unwrap().success());
        // No surviving build script retains its socket after the helper reports failure.
        assert_eq!(socket.read(&mut ready).unwrap(), 0);
        let outcomes: Value = serde_json::from_slice(
            &fs::read(fixture.root.path().join("out/outcomes.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(outcomes[0]["status"], "failed");
        assert_eq!(outcomes[1]["status"], "unattempted");
        assert!(outcomes[0]["cleanup_error"].is_null());
        assert_eq!(
            run(
                fixture.root.path(),
                "git",
                &["worktree", "list", "--porcelain"]
            )
            .matches("worktree ")
            .count(),
            1
        );
    });
}
