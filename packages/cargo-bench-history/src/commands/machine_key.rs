//! The `machine-key` command: print this machine's hardware fingerprint.
//!
//! Every engine partitions its history by this key, so it is printed to standard
//! output (clean, one line) for CI to capture and thread
//! into an `analyze` selection. Under `--verbose`, the individual factors that
//! make up the fingerprint are emitted to standard error so a change in the key
//! can be traced to the specific factor that changed.

use cbh_diag::{Reporter, ReporterExt, StderrReporter};
use cbh_probe::{
    EnvironmentProbe, HardwareProfile, SystemProbe, describe_fingerprint_components,
    resolve_machine_key,
};

use crate::{MachineKeyOptions, RunError, RunOutcome};

/// Executes the `machine-key` command against the real host.
///
/// # Errors
///
/// Never fails: hardware probing is best-effort and always yields a profile.
/// Returns a `Result` to match the shape every command handler shares.
pub(crate) async fn execute(options: &MachineKeyOptions) -> Result<RunOutcome, RunError> {
    let profile = SystemProbe::default().hardware().await;
    let reporter = StderrReporter::new(options.verbose);
    Ok(machine_key_outcome(&profile, &reporter))
}

/// Derives the machine key from `profile` and emits the outcome (pure over its
/// inputs, so it is unit-tested without probing the host).
///
/// The key becomes the outcome message (printed to standard output). The
/// fingerprint components are emitted as a diagnostic note, which the reporter
/// only surfaces (to standard error) under `--verbose`.
fn machine_key_outcome(profile: &HardwareProfile, reporter: &dyn Reporter) -> RunOutcome {
    reporter.note_with(|| {
        format!(
            "machine-key components: {}",
            describe_fingerprint_components(profile)
        )
    });
    RunOutcome::Completed {
        message: resolve_machine_key(None, profile),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use cbh_diag::RecordingReporter;

    use super::*;

    fn profile(processors: usize, memory_regions: usize, models: &[&str]) -> HardwareProfile {
        HardwareProfile {
            processors,
            memory_regions,
            processor_models: models.iter().map(|model| (*model).to_owned()).collect(),
            processor_speeds: Vec::new(),
        }
    }

    #[test]
    fn outcome_message_is_the_fingerprint() {
        let hardware = profile(8, 1, &["Test CPU 3000"]);
        let reporter = RecordingReporter::new();

        let outcome = machine_key_outcome(&hardware, &reporter);

        let RunOutcome::Completed { message } = outcome else {
            panic!("machine-key should complete: {outcome:?}");
        };
        assert_eq!(message, resolve_machine_key(None, &hardware));
    }

    #[test]
    fn verbose_note_lists_the_components() {
        let hardware = profile(8, 1, &["Test CPU 3000"]);
        let reporter = RecordingReporter::new();

        let _ = machine_key_outcome(&hardware, &reporter);

        assert!(
            reporter.contains("machine-key components:")
                && reporter.contains("processors=8")
                && reporter.contains("processor_models=Test CPU 3000"),
            "{:?}",
            reporter.notes()
        );
    }
}
