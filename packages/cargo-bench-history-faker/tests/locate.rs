//! Exercises executable ownership across real Cargo builds and process launches.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use std::{fs, thread};

use cargo_bench_history_faker::{binary_path, prepare_binary};

testing::set_allocator!();

/// Cold toolchain builds may take minutes on CI; this is only a last-chance watchdog.
const BUILD_WATCHDOG: Duration = Duration::from_mins(10);

/// Verifies a usable executable, not just a plausible artifact path.
fn assert_executable(stage: &str, path: &str) {
    // A nonzero canary proves that the faker parsed and acted on our argument.
    const EXIT_CODE: i32 = 23;

    assert!(Path::new(path).is_absolute(), "{stage}: {path}");
    let metadata = fs::metadata(path)
        .unwrap_or_else(|error| panic!("{stage}: cannot inspect {path}: {error}"));
    assert!(metadata.is_file(), "{stage}: not a file: {path}");
    let status = Command::new(path)
        .args(["--exit-code", &EXIT_CODE.to_string()])
        .status()
        .unwrap_or_else(|error| panic!("{stage}: cannot spawn {path}: {error}"));
    assert_eq!(status.code(), Some(EXIT_CODE), "{stage}: {path}");
}

#[test]
#[cfg_attr(miri, ignore = "Builds and spawns native executables.")]
fn binary_path_returns_an_existing_absolute_executable() {
    testing::with_watchdog_timeout(BUILD_WATCHDOG, || {
        assert_executable("cached locator", binary_path());
        assert_eq!(binary_path(), binary_path());
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Builds native executables and removes a real build cache."
)]
fn executable_survives_cache_removal_and_concurrent_builds() {
    testing::with_watchdog_timeout(BUILD_WATCHDOG, || {
        let root = tempfile::Builder::new()
            .prefix("faker outputs ")
            .tempdir()
            .unwrap();
        let first = prepare_binary(None, root.path());
        assert_executable("initial preparation", &first);

        let (second, third) = thread::scope(|scope| {
            let second = scope.spawn(|| prepare_binary(None, root.path()));
            let third = scope.spawn(|| {
                // A missing override must still prepare an owned executable.
                prepare_binary(
                    Some(root.path().join("missing").into_os_string()),
                    root.path(),
                )
            });
            assert_executable("concurrent preparations", &first);
            (second.join().unwrap(), third.join().unwrap())
        });
        assert_ne!(first, second);
        assert_ne!(first, third);
        assert_ne!(second, third);
        for executable in [&first, &second, &third] {
            assert_executable("completed preparations", executable);
        }

        // Force the missing-build-output interval rather than hoping to observe
        // Cargo's brief unlink/relink window. Requiring this isolated cache to
        // exist also verifies that Cargo honors the shared build-directory setting.
        fs::remove_dir_all(root.path().join("build")).unwrap();
        for executable in [&first, &second, &third] {
            assert_executable("removed intermediate cache", executable);
        }
    });
}

#[test]
#[cfg_attr(miri, ignore = "Checks an existing file and spawns the real faker.")]
fn absolute_override_skips_building() {
    testing::with_watchdog(|| {
        let root = tempfile::tempdir().unwrap();
        let unused = root.path().join("unused");
        let executable = env!("CARGO_BIN_EXE_cargo-bench-history-faker");
        let resolved = prepare_binary(Some(OsString::from(executable)), &unused);
        assert_eq!(resolved, executable);
        assert!(!unused.exists());
        assert_executable("absolute override", &resolved);
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Resolves a relative path against the real working directory."
)]
fn relative_override_becomes_absolute_without_building() {
    let root = tempfile::tempdir().unwrap();
    let unused = root.path().join("unused");
    let resolved = prepare_binary(Some(OsString::from("Cargo.toml")), &unused);
    assert_eq!(
        Path::new(&resolved),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")
    );
    assert!(Path::new(&resolved).is_file());
    assert!(!unused.exists());
}
