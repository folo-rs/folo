/// A processor identifier, used to differentiate processors in the system.
///
/// Values used are not guaranteed to be sequential/contiguous or to start from zero. The only
/// guarantee given is that different processors have different IDs.
///
/// In principle, the physical processor assigned to a given ID may even change over time due to
/// runtime changes in system hardware (e.g. a VM moving to a different host).
pub type ProcessorId = u32;

/// A processor identifier, used to differentiate processors in the system.
///
/// Values used are not guaranteed to be sequential/contiguous or to start from zero. The only
/// guarantee given is that different memory regions have different IDs.
pub type MemoryRegionId = u32;

/// Differentiates processors by their efficiency class, allowing work requiring high
/// performance to be placed on the most performant processors at the expense of energy usage.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EfficiencyClass {
    /// A processor that is optimized for energy efficiency at the expense of performance.
    Efficiency,

    /// A processor that is optimized for performance at the expense of energy efficiency.
    Performance,
}
