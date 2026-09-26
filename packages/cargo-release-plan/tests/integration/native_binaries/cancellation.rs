//! Termination signals stop the owned build tree and still clean the prepared source.

use std::io::{ErrorKind, Read, Write as _};
use std::net::TcpListener;
use std::process::Stdio;

use serde_json::json;

#[cfg(windows)]
use crate::native_binaries::cancellation_windows::spawn_controller;
use crate::native_binaries::{Fixture, SMOKE_WATCHDOG, run, write};

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
#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetConsoleCtrlHandler(handler: Option<unsafe extern "system" fn(u32) -> i32>, add: i32) -> i32;
}
#[cfg(windows)]
unsafe extern "system" fn ignore_console_event(_event: u32) -> i32 { 1 }
fn main() {
    #[cfg(windows)]
    {
        // Ignore console signals in the descendant: socket closure must demonstrate owned-tree
        // termination, not a default handler reacting directly to the controller's signal.
        // SAFETY: The callback is a static function with the Win32 signature and no borrowed state.
        let registered = unsafe { SetConsoleCtrlHandler(Some(ignore_console_event), 1) };
        assert_ne!(registered, 0);
    }
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
        let mut command = fixture.batch_command(
            &json!([fixture.binary("alpha"), fixture.binary("beta")]),
            "out",
        );
        command
            .arg("--no-upload")
            .env(
                "RELEASE_FIXTURE_SOCKET",
                listener.local_addr().unwrap().to_string(),
            )
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        #[cfg(unix)]
        let mut process = command.spawn().unwrap();
        #[cfg(windows)]
        let (mut process, mut control) = spawn_controller(&fixture, &command);
        let (mut socket, _) = listener.accept().unwrap();
        let mut ready = [0; 5];
        socket.read_exact(&mut ready).unwrap();
        assert_eq!(&ready, b"ready");
        socket.write_all(b"x").unwrap();
        let mut waiting = [0; 7];
        socket.read_exact(&mut waiting).unwrap();
        assert_eq!(&waiting, b"waiting");
        #[cfg(unix)]
        run(
            fixture.root.path(),
            "kill",
            &["-TERM", &process.id().to_string()],
        );
        #[cfg(windows)]
        control.write_all(b"x").unwrap();
        assert!(!process.wait().unwrap().success());
        // No surviving build script retains its socket after the helper reports failure.
        // Forced termination may reset a Windows socket rather than sending an orderly EOF.
        match socket.read(&mut ready) {
            Ok(length) => assert_eq!(length, 0),
            Err(error) => assert_eq!(error.kind(), ErrorKind::ConnectionReset),
        }
        let outcomes = fixture.outcomes("out");
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
