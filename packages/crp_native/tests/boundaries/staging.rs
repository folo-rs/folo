#![cfg(feature = "private-test-util")]

use std::fs;
use std::{io, mem};

use crp_native::__private::stage_executable_for_test;
use tempfile::TempDir;

#[test]
#[cfg_attr(miri, ignore = "copies and inspects real staged executable files")]
fn copied_executable_preserves_bytes_permissions_and_existing_destinations() {
    let directory = TempDir::new().unwrap();
    let executable = directory.path().join("input");
    let staged = directory.path().join("staged");
    let contents = b"representative executable content".repeat(1024);
    fs::write(&executable, &contents).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // An executable mode different from the creation default verifies metadata preservation.
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o751)).unwrap();
    }
    stage_executable_for_test(&executable, &staged, || Ok(())).unwrap();
    assert_eq!(fs::read(&staged).unwrap(), contents);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&staged).unwrap().permissions().mode(),
            fs::metadata(&executable).unwrap().permissions().mode(),
        );
    }
    fs::write(&staged, b"existing destination").unwrap();
    stage_executable_for_test(&executable, &staged, || Ok(())).unwrap_err();
    assert_eq!(fs::read(&staged).unwrap(), b"existing destination");
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "observes real staging files at cooperative checkpoints"
)]
fn interrupted_copy_does_not_publish_or_retain_private_files() {
    let directory = TempDir::new().unwrap();
    let executable = directory.path().join("input");
    let output = directory.path().join("output");
    fs::create_dir_all(&output).unwrap();
    let staged = output.join("staged");
    let contents = b"artifact bytes spanning buffered writes".repeat(1024);
    fs::write(&executable, &contents).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    }
    stage_executable_for_test(&executable, &staged, || {
        Err(io::Error::other("before copy canary").into())
    })
    .unwrap_err();
    assert_eq!(fs::read_dir(&output).unwrap().count(), 0);

    let mut started = false;
    let mut interrupted = false;
    stage_executable_for_test(&executable, &staged, || {
        // Permit entry validation, then interrupt after the owned temporary file is created.
        // Directory metadata lengths are not portable observations of an open buffered file.
        if mem::replace(&mut started, true) {
            interrupted = true;
            Err(io::Error::other("copy interruption canary").into())
        } else {
            Ok(())
        }
    })
    .unwrap_err();
    assert!(interrupted);
    assert!(!staged.exists());
    assert_eq!(fs::read_dir(&output).unwrap().count(), 0);
}

#[test]
#[cfg_attr(miri, ignore = "checks native executable input file types")]
fn invalid_copy_inputs_leave_no_staged_output() {
    let directory = TempDir::new().unwrap();
    let staged = directory.path().join("staged");
    for input in [
        directory.path().join("missing"),
        directory.path().to_owned(),
    ] {
        stage_executable_for_test(&input, &staged, || Ok(())).unwrap_err();
        assert!(!staged.exists());
    }
}

#[test]
#[cfg(unix)]
#[cfg_attr(miri, ignore = "observes actual filesystem execute permissions")]
fn staged_copy_obeys_observed_execute_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let directory = TempDir::new().unwrap();
    let input = directory.path().join("permission-input");
    let staged = directory.path().join("staged");
    fs::write(&input, b"permission fixture").unwrap();
    fs::set_permissions(&input, fs::Permissions::from_mode(0o644)).unwrap();
    // Mounted filesystems may not represent the requested mode. Validate the actual observation;
    // the in-process mode tests cover rejection independently of the host filesystem's capabilities.
    let executable = fs::metadata(&input).unwrap().permissions().mode() & 0o111 != 0;
    let result = stage_executable_for_test(&input, &staged, || Ok(()));
    assert_eq!(result.is_ok(), executable);
    assert_eq!(staged.exists(), executable);
}
