//! Native Git configuration selection precedes evidence mutation.

use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::tempdir;

use crate::fixture::hermetic_git;
use crate::harness::seeded_package;
use crate::report::{directory_alias, report_command};

#[test]
#[cfg_attr(
    miri,
    ignore = "observes native default Git attributes and ignore selection"
)]
fn report_protects_default_git_rules_and_respects_overrides() {
    for use_xdg in [false, true] {
        let fixture = seeded_package();
        let external = tempdir().unwrap();
        let home = tempdir().unwrap();
        let config = home.path().join(if use_xdg { "xdg" } else { ".config" });
        fs::create_dir_all(&config).unwrap();
        fs::create_dir_all(external.path().join("diffs")).unwrap();
        let alias = config.join("git");
        directory_alias(&external.path().join("diffs"), &alias);
        let rules = [
            ("core.attributesFile", "attributes", "*.rs text eol=lf\n"),
            ("core.excludesFile", "ignore", "ignored-canary\n"),
        ];
        for (key, name, contents) in rules {
            // Only the rule currently under assessment is enabled; the other explicit
            // empty value makes each positive and negative control independent.
            for (other_key, _, _) in rules {
                fixture.git(&["config", other_key, ""]);
            }
            fixture.git(&["config", "--unset", key]);
            let rule = external.path().join("diffs").join(name);
            fs::write(&rule, contents).unwrap();
            let mut native = hermetic_git();
            native.current_dir(fixture.path());
            let arguments = if name == "attributes" {
                vec!["check-attr", "text", "--", "packages/demo/src/lib.rs"]
            } else {
                vec!["check-ignore", "--no-index", "--verbose", "ignored-canary"]
            };
            native.args(arguments);
            isolate_git_home(
                &mut native,
                home.path(),
                use_xdg.then_some(config.as_path()),
            );
            let selected = native.output().unwrap();
            assert!(selected.status.success(), "{selected:?}");
            if name == "attributes" {
                assert!(
                    String::from_utf8(selected.stdout)
                        .unwrap()
                        .contains("text: set")
                );
            }
            let mut command = report_command(&fixture, external.path());
            isolate_git_home(
                &mut command,
                home.path(),
                use_xdg.then_some(config.as_path()),
            );
            let result = command.output().unwrap();
            assert!(!result.status.success(), "{name}: {result:?}");
            assert_eq!(fs::read_to_string(&rule).unwrap(), contents);
            assert!(!external.path().join("report.json").exists());
            assert_eq!(
                fs::canonicalize(&alias).unwrap(),
                fs::canonicalize(external.path().join("diffs")).unwrap()
            );

            // An explicit empty value and an explicit disjoint override both disable
            // this default location. Native behavior, not its pathname, controls admission.
            let override_path = home.path().join("override");
            fs::write(&override_path, "").unwrap();
            for value in ["", override_path.to_str().unwrap()] {
                fixture.git(&["config", key, value]);
                let unselected = native.output().unwrap();
                if name == "attributes" {
                    assert!(unselected.status.success(), "{unselected:?}");
                    assert!(
                        String::from_utf8(unselected.stdout)
                            .unwrap()
                            .contains("text: unspecified")
                    );
                } else {
                    assert_eq!(unselected.status.code(), Some(1), "{unselected:?}");
                }
                let result = command.output().unwrap();
                assert!(result.status.success(), "{name}: {result:?}");
                assert!(!rule.exists());
                fs::write(&rule, contents).unwrap();
            }
            fs::remove_file(external.path().join("report.json")).unwrap();
        }
    }
}

fn isolate_git_home(command: &mut Command, home: &Path, xdg: Option<&Path>) {
    // Git's isolated HOME must not move rustup/Cargo's existing installations.
    let original_home = env::home_dir().unwrap();
    command
        .env(
            "RUSTUP_HOME",
            env::var_os("RUSTUP_HOME").map_or_else(|| original_home.join(".rustup"), Into::into),
        )
        .env(
            "CARGO_HOME",
            env::var_os("CARGO_HOME").map_or_else(|| original_home.join(".cargo"), Into::into),
        )
        .env("HOME", home)
        .env("GIT_ATTR_NOSYSTEM", "1");
    if let Some(xdg) = xdg {
        command.env("XDG_CONFIG_HOME", xdg);
    } else {
        command.env_remove("XDG_CONFIG_HOME");
    }
}

#[test]
#[cfg(unix)]
#[cfg_attr(miri, ignore = "checks Git's disabled default selection without HOME")]
fn report_accepts_git_without_a_home_default() {
    let fixture = seeded_package();
    let output = tempdir().unwrap();
    let mut native = hermetic_git();
    native
        .current_dir(fixture.path())
        .env_remove("HOME")
        .env_remove("XDG_CONFIG_HOME")
        .args(["var", "GIT_ATTR_GLOBAL"]);
    let selected = native.output().unwrap();
    assert_eq!(selected.status.code(), Some(1), "{selected:?}");
    assert!(selected.stderr.is_empty());
    let mut command = report_command(&fixture, output.path());
    isolate_git_home(&mut command, output.path(), None);
    command.env_remove("HOME");
    let result = command.output().unwrap();
    assert!(result.status.success(), "{result:?}");
    assert!(output.path().join("report.json").exists());
}
