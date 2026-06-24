use crate::harness::serial;
use crate::harness::*;

#[tokio::test]
#[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot do.
async fn install_writes_config_to_the_default_path() {
    let workspace = Workspace::empty();

    let outcome = workspace.drive(&["install"]).await.unwrap();

    let RunOutcome::Completed { message } = outcome else {
        panic!("install should complete: {outcome:?}");
    };
    assert!(
        message.contains("Wrote a starter configuration"),
        "{message}"
    );
    assert!(message.contains("Next steps"), "{message}");
    assert_eq!(
        workspace.read(".cargo/bench_history.toml").as_deref(),
        Some(default_template()),
        "the default template should be written verbatim"
    );
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot do.
async fn install_never_overwrites_an_existing_config() {
    let workspace = Workspace::empty();
    workspace.drive(&["install"]).await.unwrap();

    // Hand-edit the generated file, then install again: it must be untouched.
    let edited = format!("{}\n# hand-edited\n", default_template());
    std::fs::write(workspace.root().join(".cargo/bench_history.toml"), &edited).unwrap();

    let outcome = workspace.drive(&["install"]).await.unwrap();

    let RunOutcome::Completed { message } = outcome else {
        panic!("install should complete: {outcome:?}");
    };
    assert!(message.contains("already exists"), "{message}");
    assert_eq!(
        workspace.read(".cargo/bench_history.toml").as_deref(),
        Some(edited.as_str()),
        "the hand-edited file must be left unchanged"
    );
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot do.
async fn install_honors_an_explicit_config_path() {
    let workspace = Workspace::empty();

    workspace
        .drive(&["install", "--config", "config/custom.toml"])
        .await
        .unwrap();

    assert_eq!(
        workspace.read("config/custom.toml").as_deref(),
        Some(default_template()),
        "the template should be written to the requested path"
    );
    assert!(
        workspace.read(".cargo/bench_history.toml").is_none(),
        "the default path should be left alone"
    );
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Touches the real filesystem, which Miri cannot do.
#[serial]
async fn run_entry_dispatches_without_a_target_root_override() {
    // The production `run` entry passes no target-root override; drive `install`
    // (which ignores it) through it so the public default path is exercised.
    let workspace = Workspace::empty();
    let _cwd = CwdGuard::enter(workspace.root());

    let result = run(&command_from(&["install"])).await;

    result.unwrap();
    assert!(
        workspace.read(".cargo/bench_history.toml").is_some(),
        "install via the production entry should write the default config"
    );
}
