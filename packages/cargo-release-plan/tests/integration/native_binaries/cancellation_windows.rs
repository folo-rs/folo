//! Uses an isolated console to deliver cancellation without signalling the test runner.

use std::net::{TcpListener, TcpStream};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};

use crate::native_binaries::{Fixture, command, compile_tool};

pub(crate) fn spawn_controller(fixture: &Fixture, controller: &Command) -> (Child, TcpStream) {
    // Windows console events require the sender and target to share a console. The launcher
    // owns a new console, so this also works when the test runner has no console of its own.
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    let directory = fixture.root.path().join("out/console");
    compile_tool(&directory, "console-controller", LAUNCHER);
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let mut launcher = command(
        fixture.root.path(),
        directory.join("console-controller.exe"),
    );
    launcher
        .arg(listener.local_addr().unwrap().to_string())
        .arg(controller.get_program())
        .args(controller.get_args())
        .creation_flags(CREATE_NEW_CONSOLE)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    for (name, value) in controller.get_envs() {
        if let Some(value) = value {
            launcher.env(name, value);
        } else {
            launcher.env_remove(name);
        }
    }
    let child = launcher.spawn().unwrap();
    let (control, _) = listener.accept().unwrap();
    (child, control)
}

// Only the controller's process group receives CTRL_BREAK. Dropping the control connection
// also requests cancellation, so a failing test does not leave the launcher waiting for input.
const LAUNCHER: &str = r#"
use std::io::Read;
use std::net::TcpStream;
use std::os::windows::process::CommandExt;
use std::process::{Command, ExitCode, Stdio};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GenerateConsoleCtrlEvent(event: u32, process_group: u32) -> i32;
}

fn main() -> ExitCode {
    // Win32 flags give the controller its own group within the launcher's isolated console.
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    // CTRL_BREAK targets a process group even when CTRL_C is disabled for a new group.
    const CTRL_BREAK_EVENT: u32 = 1;
    let mut arguments = std::env::args_os().skip(1);
    let address = arguments.next().unwrap();
    let program = arguments.next().unwrap();
    let mut child = Command::new(program)
        .args(arguments)
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut control = TcpStream::connect(address.to_str().unwrap()).unwrap();
    let mut request = [0];
    let _request = control.read_exact(&mut request);
    // SAFETY: The event code is supported, and the owned child's ID names the process group
    // created above in this console. The function receives no pointers or borrowed memory.
    let signalled = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child.id()) };
    if signalled == 0 {
        let error = std::io::Error::last_os_error();
        let _kill = child.kill();
        let _wait = child.wait();
        panic!("failed to signal the owned controller: {error}");
    }
    let status = child.wait().unwrap();
    if status.success() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}
"#;
