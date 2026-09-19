//! Native report-destination checks through the same parsed command surface as the CLI.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;

use cargo_bench_history::AnalysisOutcome;
use serde_json::Value;

use crate::harness::*;

#[tokio::test]
#[cfg_attr(miri, ignore = "writes reports through the real filesystem adapter")]
async fn report_outputs_reject_json_outcome_collision_before_any_write() {
    let workspace = Workspace::repo(&storage_only_config());
    fs::write(workspace.root().join("report.json"), "existing report").unwrap();

    let result = workspace
        .drive(&[
            "analyze",
            "--markdown",
            "new-parent/report.md",
            "--json",
            "report.json",
            "--outcome",
            "report.json",
        ])
        .await;

    _ = result.unwrap_err();
    assert_eq!(
        workspace.read("report.json").as_deref(),
        Some("existing report")
    );
    assert!(!workspace.root().join("new-parent").exists());
}

#[tokio::test]
#[cfg_attr(miri, ignore = "writes reports through the real filesystem adapter")]
async fn report_outputs_reject_shared_command_format_collisions() {
    let workspace = Workspace::repo(&storage_only_config());
    fs::write(workspace.root().join("report"), "existing report").unwrap();

    let result = workspace
        .drive(&[
            "list",
            "discriminants",
            "--markdown",
            "report",
            "--json",
            "./report",
        ])
        .await;

    _ = result.unwrap_err();
    assert_eq!(workspace.read("report").as_deref(), Some("existing report"));
}

#[tokio::test]
#[cfg_attr(miri, ignore = "resolves real filesystem paths")]
async fn report_outputs_reject_equivalent_relative_and_absolute_paths() {
    let workspace = Workspace::repo(&storage_only_config());
    let destination = workspace.root().join("report.json");
    fs::write(&destination, "existing JSON").unwrap();
    let paths = [
        PathBuf::from("./report.json"),
        Path::new("missing").join("..").join("report.json"),
        destination,
    ];
    for alias in paths {
        let result = workspace
            .drive(&[
                "analyze",
                "--json",
                "report.json",
                "--outcome",
                alias.to_str().unwrap(),
            ])
            .await;
        _ = result.unwrap_err();
        assert_eq!(
            workspace.read("report.json").as_deref(),
            Some("existing JSON")
        );
        assert!(!workspace.root().join("missing").exists());
    }
}

#[tokio::test]
#[cfg_attr(miri, ignore = "compares real filesystem hard links")]
async fn report_outputs_reject_existing_hard_link_aliases() {
    let workspace = Workspace::repo(&storage_only_config());
    fs::write(workspace.root().join("report.json"), "existing JSON").unwrap();
    fs::hard_link(
        workspace.root().join("report.json"),
        workspace.root().join("alias"),
    )
    .unwrap();

    let result = workspace
        .drive(&[
            "analyze",
            "--markdown",
            "earlier.md",
            "--json",
            "report.json",
            "--outcome",
            "alias",
        ])
        .await;

    _ = result.unwrap_err();
    assert_eq!(
        workspace.read("report.json").as_deref(),
        Some("existing JSON")
    );
    assert_eq!(workspace.read("alias").as_deref(), Some("existing JSON"));
    assert!(workspace.read("earlier.md").is_none());
}

#[tokio::test]
#[cfg_attr(miri, ignore = "probes actual filesystem name equivalence")]
async fn report_outputs_use_filesystem_name_equivalence_for_missing_outputs() {
    let workspace = Workspace::repo(&storage_only_config());
    let lower = workspace.root().join("report");
    let upper = workspace.root().join("REPORT");
    fs::write(&lower, "probe").unwrap();
    // Determine the actual directory's behavior instead of assuming it from the target OS.
    let names_alias = fs::read(&upper).is_ok();
    fs::remove_file(&lower).unwrap();

    let result = workspace
        .drive(&["analyze", "--json", "report", "--outcome", "REPORT"])
        .await;

    if names_alias {
        _ = result.unwrap_err();
        assert!(!lower.exists());
        assert!(!upper.exists());
    } else {
        _ = result.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(lower).unwrap()).unwrap()["outcome"],
            "nothing_in_scope"
        );
        assert_eq!(fs::read_to_string(upper).unwrap(), "nothing_in_scope");
    }
    assert!(fs::read_dir(workspace.root()).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".bench-history-destinations-")
    }));
}

#[tokio::test]
#[cfg_attr(miri, ignore = "checks real filesystem path errors")]
async fn report_outputs_leave_earlier_files_untouched_on_preflight_error() {
    let workspace = Workspace::repo(&storage_only_config());
    fs::write(workspace.root().join("report.md"), "existing Markdown").unwrap();
    fs::write(workspace.root().join("not-directory"), "existing file").unwrap();
    let result = workspace
        .drive(&[
            "analyze",
            "--markdown",
            "report.md",
            "--json",
            "new.json",
            "--outcome",
            "not-directory/outcome",
        ])
        .await;

    _ = result.unwrap_err();
    assert_eq!(
        workspace.read("report.md").as_deref(),
        Some("existing Markdown")
    );
    assert!(workspace.read("new.json").is_none());
    assert_eq!(
        workspace.read("not-directory").as_deref(),
        Some("existing file")
    );
}

#[tokio::test]
#[cfg_attr(miri, ignore = "writes reports through the real filesystem adapter")]
async fn report_outputs_write_all_distinct_formats_and_create_parents() {
    let workspace = Workspace::repo(&storage_only_config());
    let result = workspace
        .drive(&[
            "analyze",
            "--no-text",
            "--markdown",
            "nested/report.md",
            "--json",
            "nested/report.json",
            "--markdown-summary",
            "summary/report.md",
            "--outcome",
            "outcome.txt",
        ])
        .await
        .unwrap();
    assert!(matches!(
        result,
        RunOutcome::Analyzed {
            outcome: AnalysisOutcome::NothingInScope,
            ..
        }
    ));
    assert!(workspace.read("nested/report.md").unwrap().contains('#'));
    assert!(workspace.read("summary/report.md").unwrap().contains('#'));
    let json: Value = serde_json::from_str(&workspace.read("nested/report.json").unwrap()).unwrap();
    assert_eq!(json["outcome"], "nothing_in_scope");
    assert_eq!(
        workspace.read("outcome.txt").as_deref(),
        Some("nothing_in_scope")
    );
}

#[tokio::test]
#[cfg_attr(miri, ignore = "overwrites a real output file outside the workspace")]
async fn report_outputs_keep_single_output_overwrite_and_absolute_path_behavior() {
    let workspace = Workspace::repo(&storage_only_config());
    let elsewhere = Workspace::empty();
    let path = elsewhere.root().join("report.json");
    fs::write(&path, "stale").unwrap();

    workspace
        .drive(&["analyze", "--json", path.to_str().unwrap()])
        .await
        .unwrap();

    let json: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(json["outcome"], "nothing_in_scope");
    assert!(workspace.read("report.json").is_none());
}

#[cfg(unix)]
#[tokio::test]
#[cfg_attr(miri, ignore = "resolves real symlinks")]
async fn report_outputs_resolve_symlinks_before_parent_components() {
    let workspace = Workspace::repo(&storage_only_config());
    let real = workspace.root().join("real").join("child");
    fs::create_dir_all(&real).unwrap();
    symlink(&real, workspace.root().join("alias")).unwrap();
    fs::write(
        workspace.root().join("real").join("report.json"),
        "existing JSON",
    )
    .unwrap();

    for alias in ["alias/../report.json", "missing/../alias/../report.json"] {
        let result = workspace
            .drive(&[
                "analyze",
                "--markdown",
                "earlier.md",
                "--json",
                "real/report.json",
                "--outcome",
                alias,
            ])
            .await;
        _ = result.unwrap_err();
        assert_eq!(
            workspace.read("real/report.json").as_deref(),
            Some("existing JSON")
        );
        assert!(workspace.read("earlier.md").is_none());
        assert!(!workspace.root().join("missing").exists());
    }

    workspace
        .drive(&[
            "analyze",
            "--json",
            "report.json",
            "--outcome",
            "alias/../report.json",
        ])
        .await
        .unwrap();
    let json: Value = serde_json::from_str(&workspace.read("report.json").unwrap()).unwrap();
    assert_eq!(json["outcome"], "nothing_in_scope");
    assert_eq!(
        workspace.read("real/report.json").as_deref(),
        Some("nothing_in_scope")
    );
}

#[cfg(unix)]
#[tokio::test]
#[cfg_attr(miri, ignore = "resolves real symlinked directories and files")]
async fn report_outputs_reject_symlinked_parent_and_file_aliases() {
    let workspace = Workspace::repo(&storage_only_config());
    let real = workspace.root().join("real");
    fs::create_dir_all(&real).unwrap();
    symlink(&real, workspace.root().join("alias")).unwrap();
    let result = workspace
        .drive(&[
            "analyze",
            "--json",
            "real/new/report.json",
            "--outcome",
            "alias/new/report.json",
        ])
        .await;
    _ = result.unwrap_err();
    assert!(!real.join("new").exists());

    fs::write(real.join("report.json"), "existing JSON").unwrap();
    symlink(
        real.join("report.json"),
        workspace.root().join("file-alias"),
    )
    .unwrap();
    let result = workspace
        .drive(&[
            "analyze",
            "--json",
            "real/report.json",
            "--outcome",
            "file-alias",
        ])
        .await;
    _ = result.unwrap_err();
    assert_eq!(
        workspace.read("real/report.json").as_deref(),
        Some("existing JSON")
    );
}
