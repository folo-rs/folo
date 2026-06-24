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
