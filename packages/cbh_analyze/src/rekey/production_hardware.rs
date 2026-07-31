//! Hardware facts captured from the live `folohistory` benchmark store on 2026-08-01.
//!
//! Each row is a real stored run's recorded host hardware, the machine-key segment its
//! objects sit under, and the key each rendering computes from those facts. Read as a
//! table, the rows are the mapping the migration performs on the production store: the
//! retired format spread seven GitHub-hosted runner types across nine partitions, and
//! the current format gathers them back into seven.
//!
//! The stored keys were read out of the store rather than derived here, so a rendering
//! that computes a different key for a row has drifted from the history it is supposed
//! to migrate — which is the one failure `rekey` cannot detect from the inside, because
//! every subsequent decision it makes rests on the recomputation being faithful.

use cbh_model::MachineInfo;
use cbh_probe::HardwareProfile;

/// One production runner's recorded hardware and the keys that hardware maps to.
pub(crate) struct RecordedRunner {
    /// What the runner is, so a failing row names itself.
    pub(crate) description: &'static str,
    /// The processor model string the host reported, verbatim. Every processor of these
    /// runners reports the same model, so the distinct set is this one entry.
    pub(crate) processor_model: &'static str,
    /// Logical processors the host reported.
    pub(crate) processors: usize,
    /// Memory regions the host reported.
    pub(crate) memory_regions: usize,
    /// The per-processor relative-speed histogram recorded beside the run. The retired
    /// rendering hashed it; the current one records it as provenance alone.
    pub(crate) processor_speeds: &'static [(u64, usize)],
    /// The machine-key segment this runner's objects are stored under.
    pub(crate) stored_key: &'static str,
    /// The key the retired `mk2` rendering computes from these facts.
    pub(crate) legacy_key: &'static str,
    /// The key the current `mk3` rendering computes from these facts, which is the
    /// partition the migration moves this runner's history into.
    pub(crate) current_key: &'static str,
}

impl RecordedRunner {
    /// The recorded hardware as a stored run carries it.
    ///
    /// Neither rendering reads the stored fingerprint, so it is left empty rather than
    /// invented.
    pub(crate) fn machine(&self) -> MachineInfo {
        MachineInfo {
            processors: self.processors,
            memory_regions: self.memory_regions,
            processor_models: vec![self.processor_model.to_owned()],
            processor_speeds: self.processor_speeds.to_vec(),
            fingerprint: String::new(),
        }
    }

    /// The recorded hardware as the live probe reports it, so the current key comes out
    /// of the production rendering rather than a copy of it.
    pub(crate) fn profile(&self) -> HardwareProfile {
        HardwareProfile {
            processors: self.processors,
            memory_regions: self.memory_regions,
            processor_models: vec![self.processor_model.to_owned()],
            processor_speeds: self.processor_speeds.to_vec(),
        }
    }
}

/// Every captured runner, the two readings of the ARM64 Windows machine first.
pub(crate) const RECORDED_RUNNERS: &[RecordedRunner] = &[
    COBALT_100_UNIFORM_CALIBRATION,
    COBALT_100_SPLIT_CALIBRATION,
    XEON_PLATINUM_8370C,
    XEON_6973P_C,
    EPYC_7763,
    XEON_PLATINUM_8573C,
    EPYC_9V45,
    EPYC_9V74_WINDOWS,
    EPYC_9V74_LINUX,
];

/// The ARM64 Windows runner as it reads on most boots: all four Cobalt 100 processors
/// calibrated at 10678.
pub(crate) const COBALT_100_UNIFORM_CALIBRATION: RecordedRunner = RecordedRunner {
    description: "Cobalt 100 ARM64 Windows runner, all four processors at 10678",
    processor_model: "Cobalt 100",
    processors: 4,
    memory_regions: 1,
    processor_speeds: &[(10678, 4)],
    stored_key: "3fc6d40058af4b4d",
    legacy_key: "3fc6d40058af4b4d",
    current_key: "2e3ad42f4e2cd3e1",
};

/// The same ARM64 Windows runner as it reads on the boots where one of its four
/// processors calibrates at 10681 instead — three units in 10678, or 0.028%.
///
/// Nothing about the hardware differs from [`COBALT_100_UNIFORM_CALIBRATION`]; only the
/// boot-time calibration reading does. The retired rendering hashed that reading, so the
/// two boots filed one runner's history under two keys, each of them too short for the
/// detector to judge. That is why the speed histogram is provenance rather than a key
/// factor.
pub(crate) const COBALT_100_SPLIT_CALIBRATION: RecordedRunner = RecordedRunner {
    description: "Cobalt 100 ARM64 Windows runner, one of four processors at 10681",
    processor_model: "Cobalt 100",
    processors: 4,
    memory_regions: 1,
    processor_speeds: &[(10678, 3), (10681, 1)],
    stored_key: "846b57d1fb778c2a",
    legacy_key: "846b57d1fb778c2a",
    current_key: "2e3ad42f4e2cd3e1",
};

/// An Intel Xeon Platinum 8370C runner.
const XEON_PLATINUM_8370C: RecordedRunner = RecordedRunner {
    description: "Intel Xeon Platinum 8370C runner",
    processor_model: "Intel(R) Xeon(R) Platinum 8370C CPU @ 2.80GHz",
    processors: 4,
    memory_regions: 1,
    processor_speeds: &[(8774, 4)],
    stored_key: "5860e4348448e537",
    legacy_key: "5860e4348448e537",
    current_key: "2815e14084926e90",
};

/// An Intel Xeon 6973P-C runner.
const XEON_6973P_C: RecordedRunner = RecordedRunner {
    description: "Intel Xeon 6973P-C runner",
    processor_model: "Intel(R) Xeon(R) 6973P-C",
    processors: 4,
    memory_regions: 1,
    processor_speeds: &[(8168, 4)],
    stored_key: "5982674f57c32eb7",
    legacy_key: "5982674f57c32eb7",
    current_key: "9be33904f7dfee3d",
};

/// The AMD EPYC 7763 x64 Windows runner, and the one row whose objects are stored under
/// the *current* key: the run was collected after the key format changed, so its segment
/// is already the destination the other rows migrate to.
const EPYC_7763: RecordedRunner = RecordedRunner {
    description: "AMD EPYC 7763 x64 Windows runner",
    processor_model: "AMD EPYC 7763 64-Core Processor",
    processors: 4,
    memory_regions: 1,
    processor_speeds: &[(7681, 4)],
    stored_key: "8c651396431bc05b",
    legacy_key: "a401012bc82c8396",
    current_key: "8c651396431bc05b",
};

/// An Intel Xeon Platinum 8573C runner, whose model string the host reports in upper
/// case where the other Intel hosts report mixed case.
const XEON_PLATINUM_8573C: RecordedRunner = RecordedRunner {
    description: "Intel Xeon Platinum 8573C runner",
    processor_model: "INTEL(R) XEON(R) PLATINUM 8573C",
    processors: 4,
    memory_regions: 1,
    processor_speeds: &[(7225, 4)],
    stored_key: "9c4bc58040a3dc23",
    legacy_key: "9c4bc58040a3dc23",
    current_key: "e049f02fe4a55e9e",
};

/// An AMD EPYC 9V45 runner. It shares the 8155 speed reading of the EPYC 9V74 Windows
/// row, so the speed histogram carries no discriminating power the model does not.
const EPYC_9V45: RecordedRunner = RecordedRunner {
    description: "AMD EPYC 9V45 runner",
    processor_model: "AMD EPYC 9V45 96-Core Processor",
    processors: 4,
    memory_regions: 1,
    processor_speeds: &[(8155, 4)],
    stored_key: "9f3e0396ccc9d4d2",
    legacy_key: "9f3e0396ccc9d4d2",
    current_key: "e4707ad3c36e84be",
};

/// The AMD EPYC 9V74 runner as Windows reads its processor speeds.
pub(crate) const EPYC_9V74_WINDOWS: RecordedRunner = RecordedRunner {
    description: "AMD EPYC 9V74 x64 Windows runner",
    processor_model: "AMD EPYC 9V74 80-Core Processor",
    processors: 4,
    memory_regions: 1,
    processor_speeds: &[(8155, 4)],
    stored_key: "f0896fed17814f85",
    legacy_key: "f0896fed17814f85",
    current_key: "76110f7cbbb5a5e0",
};

/// The AMD EPYC 9V74 runner as Linux reads its processor speeds — 16311 against the
/// 8155 the Windows host reports for the same model, on a scale the two operating
/// systems do not even share.
pub(crate) const EPYC_9V74_LINUX: RecordedRunner = RecordedRunner {
    description: "AMD EPYC 9V74 x64 Linux runner",
    processor_model: "AMD EPYC 9V74 80-Core Processor",
    processors: 4,
    memory_regions: 1,
    processor_speeds: &[(16311, 4)],
    stored_key: "f566ff2f037beb1a",
    legacy_key: "f566ff2f037beb1a",
    current_key: "76110f7cbbb5a5e0",
};
