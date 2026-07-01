#[test]
#[cfg_attr(miri, ignore)] // Spawns a real process, which Miri cannot do.
fn binary_help_exits_success_on_stdout() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cargo-bench-history"))
        .arg("--help")
        .output()
        .unwrap();

    assert!(output.status.success(), "--help should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("run"), "help should reach stdout: {stdout}");
    assert!(
        output.stderr.is_empty(),
        "help should not write to stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[cfg_attr(miri, ignore)] // Spawns a real process, which Miri cannot do.
fn binary_parse_error_exits_failure_on_stderr() {
    // An unknown flag is a parse error whose usage text still mentions `--help`;
    // the exit code must come from the parse status, not a substring match.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cargo-bench-history"))
        .args(["run", "--frobnicate"])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "a parse error should exit non-zero"
    );
    assert!(
        output.stdout.is_empty(),
        "a parse error should not write to stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        !output.stderr.is_empty(),
        "a parse error should write a diagnostic to stderr"
    );
}

#[test]
#[cfg_attr(miri, ignore)] // Spawns a real process, which Miri cannot do.
fn binary_without_a_subcommand_prints_descriptive_help_to_stderr() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cargo-bench-history"))
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "a missing subcommand should exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Analyze stored history"),
        "no-subcommand help should describe the commands on stderr: {stderr}"
    );
    assert!(
        output.stdout.is_empty(),
        "no-subcommand help should not write to stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

/// A bare `--local` (no `=value`) selects local storage from the
/// `CARGO_BENCH_HISTORY_STORAGE` environment variable. Driven against the real
/// binary so the genuine environment edge is exercised: with the variable set,
/// storage resolves and the command proceeds past selection to fail for an
/// unrelated reason (no git repository), proving the env path was accepted.
#[test]
#[cfg_attr(miri, ignore)] // Spawns a real process, which Miri cannot do.
fn binary_bare_local_reads_the_storage_path_from_the_environment() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cargo-bench-history"))
        .args(["list", "runs", "--local"])
        .current_dir(dir.path())
        .env("CARGO_BENCH_HISTORY_STORAGE", &store)
        .output()
        .unwrap();

    // Storage resolved from the environment; the command then failed only because
    // the temp directory is not a git repository, not on storage selection.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(stderr.contains("requires a git repository"), "{stderr}");
    assert!(
        !stderr.contains("CARGO_BENCH_HISTORY_STORAGE"),
        "storage must have resolved from the environment: {stderr}"
    );
}

/// A bare `--local` with `CARGO_BENCH_HISTORY_STORAGE` unset is a storage
/// configuration error that names the environment variable, so a user learns how
/// to supply the path.
#[test]
#[cfg_attr(miri, ignore)] // Spawns a real process, which Miri cannot do.
fn binary_bare_local_without_the_environment_variable_is_an_error() {
    let dir = tempfile::tempdir().unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cargo-bench-history"))
        .args(["list", "runs", "--local"])
        .current_dir(dir.path())
        .env_remove("CARGO_BENCH_HISTORY_STORAGE")
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(stderr.contains("CARGO_BENCH_HISTORY_STORAGE"), "{stderr}");
}

/// Writes a configuration whose Azure backend sets both `account_key` and
/// `sas_token` into `dir`'s default config path. `build_storage` rejects that
/// conflicting pair the instant it is reached — before touching the network — so a
/// command driven here fails fast with the auth error once `--cache` has resolved,
/// letting the environment-edge tests prove the cache path without a live emulator.
fn write_conflicting_auth_config(dir: &std::path::Path) {
    let config_path = dir.join(".cargo").join("bench_history.toml");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(
        &config_path,
        "[project]\n\
         id = \"testproj\"\n\n\
         [storage.azure]\n\
         account = \"devstoreaccount1\"\n\
         container = \"bench-history\"\n\
         account_key = \"a2V5\"\n\
         sas_token = \"sv=2021-08-06&sig=abc\"\n",
    )
    .unwrap();
}

/// A bare `--cache` (no `=value`) reads the mirror directory from the
/// `CARGO_BENCH_HISTORY_CACHE` environment variable. Driven against the real
/// binary so the genuine environment edge is exercised: with the variable set, the
/// cache path resolves and the command proceeds to `build_storage`, which then
/// fails only for the deliberately conflicting Azure auth — not on the cache-unset
/// path — proving the environment value was accepted.
#[test]
#[cfg_attr(miri, ignore)] // Spawns a real process, which Miri cannot do.
fn binary_bare_cache_reads_the_cache_path_from_the_environment() {
    let dir = tempfile::tempdir().unwrap();
    write_conflicting_auth_config(dir.path());
    let cache = dir.path().join("cache");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cargo-bench-history"))
        .args(["list", "runs", "--cache"])
        .current_dir(dir.path())
        .env("CARGO_BENCH_HISTORY_CACHE", &cache)
        .output()
        .unwrap();

    // Reaching the conflicting-auth error proves `build_storage` ran, so the cache
    // path resolved from the environment rather than failing as unset first.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(stderr.contains("set only one of"), "{stderr}");
    assert!(
        !stderr.contains("CARGO_BENCH_HISTORY_CACHE"),
        "the cache must have resolved from the environment: {stderr}"
    );
}

/// A bare `--cache` with `CARGO_BENCH_HISTORY_CACHE` unset is a cache
/// configuration error that names the environment variable, so a user learns how
/// to supply the directory.
#[test]
#[cfg_attr(miri, ignore)] // Spawns a real process, which Miri cannot do.
fn binary_bare_cache_without_the_environment_variable_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    write_conflicting_auth_config(dir.path());

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cargo-bench-history"))
        .args(["list", "runs", "--cache"])
        .current_dir(dir.path())
        .env_remove("CARGO_BENCH_HISTORY_CACHE")
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(stderr.contains("CARGO_BENCH_HISTORY_CACHE"), "{stderr}");
}
