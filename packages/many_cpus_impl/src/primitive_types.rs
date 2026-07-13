use std::fmt::{self, Display};
use std::num::NonZero;

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

/// A relative indicator of a processor's nominal speed, used to distinguish processors on the
/// same system more finely than [`EfficiencyClass`] alone.
///
/// The intended use is *identification*: on a heterogeneous system this value helps tell apart
/// processors of different underlying types (for example a cluster of fast cores from a cluster
/// of slower cores) that would otherwise look identical when described only by their memory
/// region and efficiency class. Processors of the same underlying type report the same value, so
/// this does not uniquely identify an individual processor - use [`Processor::id()`][1] for that.
///
/// # Comparability
///
/// The value is only meaningful *within a single system*, where a larger value indicates a
/// nominally faster processor. It is **not** comparable across different systems or operating
/// systems because the underlying platform metrics differ:
///
/// * On Linux the value is derived from the `bogomips` reported by `/proc/cpuinfo`.
/// * On Windows the value is the nominal maximum clock frequency in MHz.
/// * On platforms without a suitable metric (including the fallback platform and when running
///   under Miri) every processor reports the same synthetic value.
///
/// Because the metrics differ, never compare a `RelativeSpeed` obtained on one machine to one
/// obtained on another, and never interpret the number as a physical unit.
///
/// [1]: crate::Processor::id
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RelativeSpeed(NonZero<u32>);

impl RelativeSpeed {
    /// The value reported when no real speed information is available for a processor.
    pub(crate) const SYNTHETIC: Self = Self(NonZero::<u32>::MIN);

    /// Constructs a value from a raw platform metric, mapping a missing metric (zero) to the
    /// synthetic minimum so the value is always non-zero.
    #[must_use]
    pub(crate) fn from_raw(value: u32) -> Self {
        match NonZero::new(value) {
            Some(value) => Self(value),
            None => Self::SYNTHETIC,
        }
    }

    /// The underlying numeric value.
    ///
    /// This is only meaningful for comparing or identifying processors on the same system - see
    /// the [type-level documentation][Self] for details.
    #[must_use]
    pub fn as_u32(self) -> u32 {
        self.0.get()
    }
}

impl Display for RelativeSpeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(EfficiencyClass: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(RelativeSpeed: UnwindSafe, RefUnwindSafe);

    #[test]
    fn relative_speed_from_raw_preserves_positive_value() {
        assert_eq!(RelativeSpeed::from_raw(4890).as_u32(), 4890);
    }

    #[test]
    fn relative_speed_from_raw_maps_zero_to_synthetic() {
        assert_eq!(RelativeSpeed::from_raw(0), RelativeSpeed::SYNTHETIC);
        assert_eq!(RelativeSpeed::from_raw(0).as_u32(), 1);
    }

    #[test]
    fn relative_speed_orders_by_value() {
        assert!(RelativeSpeed::from_raw(2400) < RelativeSpeed::from_raw(3600));
    }

    #[test]
    fn relative_speed_display_writes_the_number() {
        assert_eq!(format!("{}", RelativeSpeed::from_raw(3600)), "3600");
    }
}
