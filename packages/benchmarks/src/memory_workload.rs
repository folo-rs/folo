use std::env::args_os;
use std::ffi::OsString;
use std::sync::LazyLock;

/// Selects reduced memory inputs for Criterion smoke tests.
///
/// Measurement and profiling runs retain the requested count. Use this during
/// payload setup, keeping registered scenario names independent of smoke scaling.
pub fn memory_workload_size(measurement_count: usize) -> usize {
    static WORKLOAD: LazyLock<MemoryWorkload> =
        LazyLock::new(|| MemoryWorkload::from_args(args_os().skip(1)));

    WORKLOAD.size(measurement_count)
}

/// Separates Criterion's execution mode from Cargo's compilation profile.
///
/// Cargo enables `cfg(test)` even for optimized benchmark executables. Mirroring
/// Criterion's runtime mode selection keeps those measurements full-sized.
enum MemoryWorkload {
    Smoke,
    Measurement,
}

impl MemoryWorkload {
    fn from_args(args: impl IntoIterator<Item = OsString>) -> Self {
        let mut bench = false;
        let mut test = false;

        // Criterion treats arguments after the separator as filters, not mode flags.
        for arg in args.into_iter().take_while(|arg| arg != "--") {
            bench |= arg == "--bench";
            test |= arg == "--test";
        }

        // Criterion defaults to test mode without --bench; --test also overrides --bench.
        if test || !bench {
            Self::Smoke
        } else {
            Self::Measurement
        }
    }

    fn size(&self, measurement_count: usize) -> usize {
        // Exercise nonempty maps and header collections without exploratory-scale allocation.
        const SMOKE_ENTRY_COUNT: usize = 10;

        match self {
            Self::Smoke => measurement_count.min(SMOKE_ENTRY_COUNT),
            Self::Measurement => measurement_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_uses_smoke_workload() {
        // Cargo's unit-test invocation also selects Criterion's default smoke mode.
        assert_eq!(memory_workload_size(64 * 1024), 10);
        assert_eq!(memory_workload_size(16 * 128 * 1024), 10);
        assert_eq!(memory_workload_size(10_000), 10);
    }

    #[test]
    fn criterion_modes_select_workload_sizes() {
        let cases: &[(&[&str], bool)] = &[
            (&[], true),
            (&["--test"], true),
            (&["--bench"], false),
            (&["--bench", "--test"], true),
            (&["--test", "--bench"], true),
            (&["--bench", "--profile-time", "1"], false),
            (&["--bench", "--exact", "scenario"], false),
            (&["--bench", "--test", "--exact", "scenario"], true),
            (&["--", "--bench"], true),
            (&["--bench", "--", "--test"], false),
        ];

        for &(args, smoke) in cases {
            let workload = MemoryWorkload::from_args(args.iter().map(OsString::from));

            // Distinct representative counts cover the small and large allocation paths.
            assert_eq!(workload.size(64 * 1024), if smoke { 10 } else { 64 * 1024 });
            assert_eq!(
                workload.size(16 * 128 * 1024),
                if smoke { 10 } else { 16 * 128 * 1024 }
            );
            assert_eq!(workload.size(10_000), if smoke { 10 } else { 10_000 });
        }
    }

    #[test]
    fn smoke_does_not_enlarge_inputs() {
        let workload = MemoryWorkload::from_args([]);

        for count in 0..=10 {
            assert_eq!(workload.size(count), count);
        }
    }
}
