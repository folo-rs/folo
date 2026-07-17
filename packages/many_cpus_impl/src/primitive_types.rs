// Only the real Linux and Windows platforms scale OS speed metrics by pi (see
// `RelativeSpeed::from_os_metric`); everything else reports fallback speeds, so this import is
// gated to those builds to avoid an unused-import warning elsewhere (for example under Miri).
#[cfg(any(test, all(not(miri), any(target_os = "linux", windows))))]
use std::f64::consts::PI;
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
/// systems, and it is not a physical unit - never interpret it as a clock frequency or compare
/// a `RelativeSpeed` obtained on one machine to one obtained on another.
///
/// [1]: crate::Processor::id
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RelativeSpeed(NonZero<u64>);

impl RelativeSpeed {
    /// The value reported for a processor whose nominal speed the operating system does not
    /// disclose.
    ///
    /// This is a sentinel (`u64::MAX`) chosen to be unmistakably not a real speed reading, so it
    /// cannot be confused with the small values that genuine metrics or tests produce. Nothing may
    /// depend on the specific value.
    pub(crate) const UNDETERMINED: Self = Self(NonZero::<u64>::MAX);

    /// Constructs a value from a raw OS-reported speed metric, such as the Linux `bogomips` or the
    /// Windows nominal maximum clock frequency in MHz.
    ///
    /// A `RelativeSpeed` is an abstract relative indicator, not a physical clock-frequency reading,
    /// so the raw metric is scaled by pi before it is stored. This removes any obvious numeric
    /// resemblance to the underlying MHz-style figure while preserving the ordering between
    /// processors. A missing metric (zero) has no real speed behind it and maps to a fallback value.
    // Only the real Linux and Windows platforms feed OS metrics in; the fallback (used on other
    // platforms and under Miri) reports fallback speeds, so gate this out there to avoid dead code.
    #[cfg(any(test, all(not(miri), any(target_os = "linux", windows))))]
    #[must_use]
    pub(crate) fn from_os_metric(metric: u32) -> Self {
        // A u32 scaled by pi is non-negative and at most about 1.35e10, far below u64::MAX, so the
        // cast below only drops the fractional part (the truncation we intend) without losing the
        // sign or overflowing.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a u32 scaled by pi is non-negative and far below u64::MAX"
        )]
        let scaled = (f64::from(metric) * PI) as u64;

        Self::from_raw(scaled)
    }

    /// Constructs a value from a raw numeric value. A missing value (zero) has no real speed metric
    /// behind it, so it maps to the `UNDETERMINED` sentinel to keep the value non-zero.
    #[must_use]
    pub(crate) fn from_raw(value: u64) -> Self {
        match NonZero::new(value) {
            Some(value) => Self(value),
            None => Self::UNDETERMINED,
        }
    }

    /// The underlying numeric value.
    ///
    /// This is only meaningful for comparing or identifying processors on the same system - see
    /// the [type-level documentation][Self] for details.
    #[must_use]
    pub fn as_u64(self) -> u64 {
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
        assert_eq!(RelativeSpeed::from_raw(4890).as_u64(), 4890);
    }

    #[test]
    fn relative_speed_from_raw_maps_zero_to_undetermined() {
        assert_eq!(RelativeSpeed::from_raw(0), RelativeSpeed::UNDETERMINED);
    }

    #[test]
    fn relative_speed_from_os_metric_scales_by_pi() {
        // The raw metric is multiplied by pi, then truncated: 1000 * pi = 3141.59..., 2000 * pi =
        // 6283.18.... This deliberately makes the stored number unlike the raw MHz-style figure.
        assert_eq!(RelativeSpeed::from_os_metric(1000).as_u64(), 3141);
        assert_eq!(RelativeSpeed::from_os_metric(2000).as_u64(), 6283);
    }

    #[test]
    fn relative_speed_from_os_metric_maps_zero_to_undetermined() {
        assert_eq!(
            RelativeSpeed::from_os_metric(0),
            RelativeSpeed::UNDETERMINED
        );
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
