/// Identifies a specific processor.
///
/// This will match the numeric identifier used by standard tooling of the operating system.
///
/// It is important to highlight that the values used are not guaranteed to be sequential/contiguous
/// or to start from zero (aspects that are also not guaranteed by operating system tooling).
pub type ProcessorId = u32;

/// Identifies a specific memory region.
///
/// This will match the numeric identifier used by standard tooling of the operating system.
///
/// It is important to highlight that the values used are not guaranteed to be sequential/contiguous
/// or to start from zero (aspects that are also not guaranteed by operating system tooling).
pub type MemoryRegionId = u32;

/// Differentiates processors on the performance-efficiency axis.
///
/// The idea behind this classification is that slower processors tend to be more energy-efficient,
/// so we distinguish processors that should be preferred to get processing done fast from processors
/// that should be preferred to conserve energy.
///
/// This is a relative measurement - the fastest processors in a system are always
/// considered performance processors, with slower ones considered efficiency processors.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "mirroring two-tier structure of platform APIs"
)]
pub enum EfficiencyClass {
    /// A processor that is optimized for energy efficiency at the expense of performance.
    Efficiency,

    /// A processor that is optimized for performance at the expense of energy efficiency.
    Performance,
}
