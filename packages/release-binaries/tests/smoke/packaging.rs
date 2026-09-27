//! One native wiring smoke connects the private run protocol to a usable staged artifact.

use std::fmt::Write as _;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{Fixture, SMOKE_WATCHDOG, assert_success, command, run, write};

#[test]
fn private_run_stages_a_usable_tagged_binary() {
    testing::with_watchdog_timeout(SMOKE_WATCHDOG, stages_tagged_binaries);
}

fn stages_tagged_binaries() {
    let fixture = Fixture::new();
    let result = fixture.execute(&json!([fixture.binary("alpha")]), "out");
    assert_success(&result);
    let outcomes: Value =
        serde_json::from_slice(&fs::read(fixture.root.path().join("out/outcomes.json")).unwrap())
            .unwrap();
    assert_eq!(outcomes.as_array().unwrap().len(), 1);
    for outcome in outcomes.as_array().unwrap() {
        assert_eq!(outcome["status"], "staged-only");
        assert_eq!(outcome["binary"]["source_sha"], fixture.source);
    }
    let name = "alpha";
    let base = format!("{name}-v1.0.0-{}", fixture.triple);
    let staging = fixture.root.path().join("out").join(&base);
    let archive = staging.join(format!("{base}.zip"));
    let checksum = fs::read_to_string(staging.join(format!("{base}.sha256"))).unwrap();
    let mut digest = String::new();
    for byte in Sha256::digest(fs::read(&archive).unwrap()) {
        write!(digest, "{byte:02x}").unwrap();
    }
    // Independently verify the GNU-compatible binary/text marker for the host platform.
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
    let unpacked = staging.join("unpacked");
    // .NET independently reads and extracts the Rust-written ZIP on every test platform.
    write(
        &staging,
        "inspect-archive.ps1",
        "
param([string] $Archive, [string] $Destination)
$ErrorActionPreference = 'Stop'
$z = [IO.Compression.ZipFile]::OpenRead($Archive)
try {
    ConvertTo-Json -InputObject @($z.Entries | ForEach-Object {
        @{ name = $_.FullName; attributes = $_.ExternalAttributes }
    }) -Compress
} finally { $z.Dispose() }
[IO.Compression.ZipFile]::ExtractToDirectory($Archive, $Destination)
",
    );
    let inspection = command(&staging, "pwsh")
        .args([
            "-NoProfile",
            "-File",
            "inspect-archive.ps1",
            "-Archive",
            archive_argument,
            "-Destination",
            unpacked.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_success(&inspection);
    let entries: Value = serde_json::from_slice(&inspection.stdout).unwrap();
    assert_eq!(entries.as_array().unwrap().len(), 1);
    assert_eq!(entries[0]["name"], binary);
    // ZIP stores Unix mode in the upper half of external attributes; retain an execute bit.
    #[cfg(unix)]
    assert_ne!(
        (entries[0]["attributes"].as_i64().unwrap() >> 16) & 0o111,
        0
    );
    assert_eq!(
        fs::read(unpacked.join(&binary)).unwrap(),
        fs::read(staging.join(&binary)).unwrap()
    );
    // Apply only the mode read and checked from the archive, not a fixture default.
    #[cfg(unix)]
    fs::set_permissions(
        unpacked.join(&binary),
        fs::Permissions::from_mode(
            u32::try_from((entries[0]["attributes"].as_i64().unwrap() >> 16) & 0o777).unwrap(),
        ),
    )
    .unwrap();
    assert_eq!(run(&unpacked, unpacked.join(&binary), &[]).trim(), "tagged");
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
