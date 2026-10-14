//! Read-only follow-up commands retain the actual query location and resolved history.

use std::fmt::Write as _;
use std::path::PathBuf;

use cbh_detect::{AnalysisMode, Finding};

/// The resolved filesystem inputs needed to repeat a query.
#[derive(Clone, Debug)]
pub struct InspectionLocation {
    /// Working directory also determines an unconfigured project identity.
    pub directory: PathBuf,
    /// Repository used to resolve topology.
    pub repo: PathBuf,
    /// Explicit configuration override; otherwise the directory's default applies.
    pub config: Option<PathBuf>,
    /// Resolved local backend, including an environment-selected path.
    pub local: Option<PathBuf>,
}

/// Selection evidence for read-only inspection, not a new analysis policy.
#[derive(Clone, Copy, Debug)]
pub struct Inspection<'a> {
    /// Actual directory, configuration and storage selection.
    pub location: &'a InspectionLocation,
    /// Resolved base commit, pinned independently of the context.
    pub base: &'a str,
    /// Absolute cutoff, rather than a relative duration that would move over time.
    pub since: &'a str,
    /// Whether the query excluded dirty observations.
    pub no_dirty: bool,
    /// Whether current values came from collection files rather than shared storage.
    pub exact_current: bool,
}

impl Inspection<'_> {
    /// Produces separately labelled shell commands, refusing lossy filesystem identities.
    pub(crate) fn commands(
        self,
        finding: &Finding,
        context: &str,
        mode: AnalysisMode,
    ) -> Option<Vec<(&'static str, String)>> {
        let directory = self.location.directory.to_str()?;
        let mut args = vec![
            ("--repo", self.location.repo.to_str()?.to_owned()),
            ("--context", context.to_owned()),
            ("--base", self.base.to_owned()),
            ("--engine", finding.set.engine.as_str().to_owned()),
            ("--target-triple", finding.set.target_triple.to_string()),
            ("--machine-key", finding.set.machine_key.to_string()),
            ("--benchmark", finding.id.qualified()),
            ("--metric", finding.kind.as_str().to_owned()),
        ];
        args.push(("--since", self.since.to_owned()));
        if let Some(config) = &self.location.config {
            args.push(("--config", config.to_str()?.to_owned()));
        }
        if let Some(local) = &self.location.local {
            args.push(("--local", local.to_str()?.to_owned()));
        }
        // Terminal controls cannot be printed safely as a copyable literal command.
        if directory.chars().any(char::is_control)
            || args
                .iter()
                .any(|(_, value)| value.chars().any(char::is_control))
        {
            return None;
        }
        let mut commands = Vec::new();
        for shell in [Shell::PowerShell, Shell::Posix] {
            commands.push((
                shell.label(),
                shell.command(directory, &args, self.no_dirty),
            ));
            if mode == AnalysisMode::Branch {
                // Examine follows the context's first-parent line. The current base may
                // have advanced independently, so it needs its own explicit inspection.
                let base_args = args
                    .iter()
                    .map(|(flag, value)| {
                        (
                            *flag,
                            if *flag == "--context" {
                                self.base.to_owned()
                            } else {
                                value.clone()
                            },
                        )
                    })
                    .collect::<Vec<_>>();
                commands.push((
                    shell.base_label(),
                    shell.command(directory, &base_args, true),
                ));
            }
        }
        Some(commands)
    }
}

/// Shell-specific quoting is explicit; no command is executed by the renderer.
#[derive(Clone, Copy)]
enum Shell {
    PowerShell,
    Posix,
}

impl Shell {
    fn quote(self, value: &str) -> String {
        let escaped = match self {
            Self::PowerShell => {
                let mut escaped = String::new();
                for ch in value.chars() {
                    escaped.push(ch);
                    // PowerShell recognizes typographic single quotes as delimiters too.
                    if matches!(ch, '\'' | '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}') {
                        escaped.push(ch);
                    }
                }
                escaped
            }
            Self::Posix => value.replace('\'', "'\\''"),
        };
        format!("'{escaped}'")
    }

    fn label(self) -> &'static str {
        match self {
            Self::PowerShell => "PowerShell 7: stored context observations",
            Self::Posix => "POSIX shell: stored context observations",
        }
    }

    fn base_label(self) -> &'static str {
        match self {
            Self::PowerShell => "PowerShell 7: clean base history",
            Self::Posix => "POSIX shell: clean base history",
        }
    }

    fn command(self, directory: &str, args: &[(&str, String)], no_dirty: bool) -> String {
        let cd = match self {
            Self::PowerShell => "Set-Location -LiteralPath",
            Self::Posix => "cd --",
        };
        let mut command = format!(
            "{cd} {} && cargo bench-history examine",
            self.quote(directory)
        );
        for (flag, value) in args {
            // The equals form protects leading hyphens as well as shell metacharacters.
            write!(command, " {flag}={}", self.quote(value))
                .expect("writing to a String never fails");
        }
        if no_dirty {
            command.push_str(" --no-dirty");
        }
        command
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(InspectionLocation: Send, Sync, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(Inspection<'_>: Send, Sync, UnwindSafe, RefUnwindSafe);

    #[test]
    fn shell_arguments_remain_literal_including_quotes_and_leading_hyphens() {
        let value = "-a'b $HOME; `echo x` | <tag>\nnext";
        assert_eq!(
            Shell::PowerShell.quote(value),
            "'-a''b $HOME; `echo x` | <tag>\nnext'"
        );
        assert_eq!(
            Shell::Posix.quote(value),
            "'-a'\\''b $HOME; `echo x` | <tag>\nnext'"
        );
        let command =
            Shell::PowerShell.command("C:\\work", &[("--benchmark", value.to_owned())], true);
        assert!(command.contains("--benchmark='-a''b"));
        assert!(command.ends_with(" --no-dirty"));
        assert_eq!(
            Shell::PowerShell.quote("a\u{2019}b"),
            "'a\u{2019}\u{2019}b'"
        );
    }
}
