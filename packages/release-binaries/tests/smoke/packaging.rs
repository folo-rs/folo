//! Staging contracts: shared build output, portable archives and independent item failures.

use std::env::consts::EXE_SUFFIX;
use std::fmt::Write as _;
use std::fs;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{Fixture, SMOKE_WATCHDOG, assert_success, command, run, write};

#[test]
fn stages_tagged_binaries_with_shared_output_and_root_archives() {
    testing::with_watchdog_timeout(SMOKE_WATCHDOG, stages_tagged_binaries);
}

fn stages_tagged_binaries() {
    let fixture = Fixture::new();
    let result = fixture.execute(
        &json!([fixture.binary("alpha"), fixture.binary("beta")]),
        "out",
    );
    assert_success(&result);
    let outcomes: Value =
        serde_json::from_slice(&fs::read(fixture.root.path().join("out/outcomes.json")).unwrap())
            .unwrap();
    assert_eq!(outcomes.as_array().unwrap().len(), 2);
    for outcome in outcomes.as_array().unwrap() {
        assert_eq!(outcome["status"], "staged-only");
        assert_eq!(outcome["binary"]["source_sha"], fixture.source);
    }
    for name in ["alpha", "beta"] {
        let base = format!("{name}-v1.0.0-{}", fixture.triple);
        let staging = fixture.root.path().join("out").join(&base);
        let archive = staging.join(format!("{base}.zip"));
        let checksum = fs::read_to_string(staging.join(format!("{base}.sha256"))).unwrap();
        let mut digest = String::new();
        for byte in Sha256::digest(fs::read(&archive).unwrap()) {
            write!(digest, "{byte:02x}").unwrap();
        }
        assert_eq!(
            checksum,
            format!(
                "{digest} {}{base}.zip\n",
                if cfg!(windows) { "*" } else { " " }
            )
        );
        let binary = if cfg!(windows) {
            format!("{name}-bin.exe")
        } else {
            format!("{name}-bin")
        };
        let archive_argument = archive.to_str().unwrap();
        // Inspect through the platform's existing archive implementation, not an in-repo codec.
        write(
            &staging,
            "inspect-archive.ps1",
            "
param([string] $Archive)
$ErrorActionPreference = 'Stop'
$z = [IO.Compression.ZipFile]::OpenRead($Archive)
try {
    ConvertTo-Json -InputObject @($z.Entries | ForEach-Object {
        @{ name = $_.FullName; attributes = $_.ExternalAttributes }
    }) -Compress
} finally { $z.Dispose() }
",
        );
        let inspection = command(&staging, "pwsh")
            .args([
                "-NoProfile",
                "-File",
                "inspect-archive.ps1",
                "-Archive",
                archive_argument,
            ])
            .output()
            .unwrap();
        assert_success(&inspection);
        let entries: Value = serde_json::from_slice(&inspection.stdout).unwrap();
        assert_eq!(entries.as_array().unwrap().len(), 1);
        assert_eq!(entries[0]["name"], binary);
        #[cfg(unix)]
        assert_ne!(
            (entries[0]["attributes"].as_i64().unwrap() >> 16) & 0o111,
            0
        );
        let unpacked = staging.join("unpacked");
        fs::create_dir_all(&unpacked).unwrap();
        #[cfg(windows)]
        run(&unpacked, "7za", &["x", "-y", archive_argument]);
        #[cfg(unix)]
        run(&unpacked, "unzip", &["-q", archive_argument]);
        assert_eq!(run(&unpacked, unpacked.join(&binary), &[]).trim(), "tagged");
    }
    // Both independent builds use the fixture controller's shared target tree.
    assert_eq!(
        String::from_utf8_lossy(&result.stderr)
            .matches("Compiling shared ")
            .count(),
        1
    );
    let deps = fixture
        .root
        .path()
        .join("target")
        .join(&fixture.triple)
        .join("release/deps");
    assert!(fs::read_dir(deps).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("libshared-")
    }));
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
    assert!(run(fixture.root.path(), "git", &["status", "--porcelain"]).is_empty());
}

#[test]
fn failed_item_does_not_publish_or_prevent_independent_staging() {
    testing::with_watchdog_timeout(SMOKE_WATCHDOG, stages_after_item_failure);
}

fn stages_after_item_failure() {
    let fixture = Fixture::new();
    let mut invalid = fixture.binary("alpha");
    invalid["version"] = "9.0.0".into();
    invalid["tag"] = "alpha-v9.0.0".into();
    let result = fixture.execute(&json!([invalid, fixture.binary("beta")]), "out");
    assert!(!result.status.success());
    let outcomes: Value =
        serde_json::from_slice(&fs::read(fixture.root.path().join("out/outcomes.json")).unwrap())
            .unwrap();
    assert_eq!(outcomes[0]["status"], "failed");
    assert_eq!(outcomes[0]["stage"], "build");
    assert_eq!(outcomes[1]["status"], "staged-only");
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
}

#[test]
fn package_features_do_not_leak_across_separate_builds() {
    testing::with_watchdog_timeout(SMOKE_WATCHDOG, || {
        let mut fixture = Fixture::new();
        write(
            fixture.root.path(),
            "shared/Cargo.toml",
            r#"
[package]
name = "shared"
version = "1.0.0"
edition = "2024"
[features]
extra = []
"#,
        );
        write(
            fixture.root.path(),
            "shared/src/lib.rs",
            "pub fn message() -> &'static str { if cfg!(feature = \"extra\") { \"extra\" } else { \"base\" } }\n",
        );
        let manifest = fixture.root.path().join("alpha/Cargo.toml");
        let contents = fs::read_to_string(&manifest).unwrap().replace(
            "shared = { path = \"../shared\" }",
            "shared = { path = \"../shared\", features = [\"extra\"] }",
        );
        fs::write(manifest, contents).unwrap();
        fixture.commit_source();
        let result = fixture.execute(
            &json!([fixture.binary("alpha"), fixture.binary("beta")]),
            "out",
        );
        assert_success(&result);
        for (name, expected) in [("alpha", "extra"), ("beta", "base")] {
            let base = format!("{name}-v1.0.0-{}", fixture.triple);
            let filename = format!("{name}-bin{EXE_SUFFIX}");
            let staged = fixture.root.path().join("out").join(base);
            assert_eq!(run(&staged, staged.join(filename), &[]).trim(), expected);
        }
    });
}
