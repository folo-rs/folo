/// A processor identifier, used to differentiate processors in the system. This will match
/// the numeric identifier used by standard tooling of the operating system.
///
/// It is important to highlight that the values used are not guaranteed to be sequential/contiguous
/// or to start from zero (aspects that are also not guaranteed by operating system tooling).
pub type ProcessorId = u32;

/// A memory region identifier, used to differentiate memory regions in the system. This will match
/// the numeric identifier used by standard tooling of the operating system.
///
/// It is important to highlight that the values used are not guaranteed to be sequential/contiguous
/// or to start from zero (aspects that are also not guaranteed by operating system tooling).
pub type MemoryRegionId = u32;

/// Differentiates processors by their efficiency class, allowing work requiring high
/// performance to be placed on the most performant processors at the expense of energy usage.
///
/// This is a relative measurement - the most performant processors in a system are always
/// considered performance processors, with less performant ones considered efficiency processors.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EfficiencyClass {
    /// A processor that is optimized for energy efficiency at the expense of performance.
    Efficiency,

    /// A processor that is optimized for performance at the expense of energy efficiency.
    Performance,
}
