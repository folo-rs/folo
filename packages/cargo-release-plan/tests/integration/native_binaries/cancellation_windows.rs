//! Uses an isolated console to deliver cancellation without signalling the test runner.

use std::os::windows::process::CommandExt;
use std::process::{Child, ChildStdin, Command, Stdio};

use crate::native_binaries::{Fixture, command, compile_tool};

pub(crate) fn spawn_controller(fixture: &Fixture, controller: &Command) -> (Child, ChildStdin) {
    // Windows console events require the sender and target to share a console. The launcher
    // owns a new console, so this also works when the test runner has no console of its own.
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    let directory = fixture.root.path().join("out/console");
    compile_tool(&directory, "console-controller", LAUNCHER);
    let mut launcher = command(
        fixture.root.path(),
        directory.join("console-controller.exe"),
    );
    launcher
        .arg(controller.get_program())
        .args(controller.get_args())
        .creation_flags(CREATE_NEW_CONSOLE)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    for (name, value) in controller.get_envs() {
        if let Some(value) = value {
            launcher.env(name, value);
        } else {
            launcher.env_remove(name);
        }
    }
    let mut child = launcher.spawn().unwrap();
    let control = child.stdin.take().unwrap();
    (child, control)
}

// Only the controller's process group receives CTRL_BREAK. The launcher observes child exit
// independently of stdin; an abort byte or closed input forcibly cleans up its owned child.
const LAUNCHER: &str = r#"
use std::io::Read;
use std::os::windows::io::{AsHandle, AsRawHandle};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::{Arc, Mutex};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GenerateConsoleCtrlEvent(event: u32, process_group: u32) -> i32;
    fn TerminateProcess(process: *mut std::ffi::c_void, exit_code: u32) -> i32;
}

// The fixture owns this child even if launcher setup fails after spawning it.
struct OwnedController(Child);
impl Drop for OwnedController {
    fn drop(&mut self) {
        let _kill = self.0.kill();
        let _wait = self.0.wait();
    }
}

fn main() -> ExitCode {
    // Win32 flags give the controller its own group within the launcher's isolated console.
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    // CTRL_BREAK targets a process group even when CTRL_C is disabled for a new group.
    const CTRL_BREAK_EVENT: u32 = 1;
    let mut arguments = std::env::args_os().skip(1);
    let program = arguments.next().unwrap();
    let mut child = OwnedController(Command::new(program)
        .args(arguments)
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap());
    let process_group = child.0.id();
    let handle = child.0.as_handle().try_clone_to_owned().unwrap();
    let finished = Arc::new(Mutex::new(false));
    std::thread::spawn({
        let finished = Arc::clone(&finished);
        move || {
            let mut input = std::io::stdin().lock();
            loop {
                let mut request = [0];
                let cancel = input.read_exact(&mut request).is_ok() && request == *b"c";
                let completed = finished.lock().unwrap();
                if *completed { return; }
                let terminate = if cancel {
                    // SAFETY: The supported event targets our owned child's group in this console.
                    // The completed lock and owned process handle keep its lifetime bounded.
                    let signalled = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, process_group) };
                    signalled == 0
                } else {
                    true
                };
                if terminate {
                    // SAFETY: This duplicated live handle belongs only to the child we spawned.
                    // A nonzero fixture exit code marks aborted or failed signal delivery.
                    let stopped = unsafe { TerminateProcess(handle.as_raw_handle(), 1) };
                    if stopped == 0 {
                        eprintln!("failed to abort owned controller: {}", std::io::Error::last_os_error());
                    }
                    return;
                }
            }
        }
    });
    let status = child.0.wait().unwrap();
    *finished.lock().unwrap() = true;
    if status.success() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}
"#;
