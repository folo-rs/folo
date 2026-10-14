//! Human quantities use the model's unit, never a second metric-to-unit mapping.

use cbh_model::MetricKind;

use crate::format_value;

/// Formats a metric in a readable unit without changing its stored value.
#[must_use]
pub fn format_quantity(kind: MetricKind, value: f64) -> String {
    QuantityScale::new(kind, [value]).quantity(value)
}

/// One scale for a comparison and its chart, so their numbers remain comparable.
#[derive(Clone, Copy, Debug)]
pub(crate) struct QuantityScale {
    pub(crate) unit: &'static str,
    divisor: f64,
}

impl QuantityScale {
    pub(crate) fn new(kind: MetricKind, values: impl IntoIterator<Item = f64>) -> Self {
        let magnitude = values
            .into_iter()
            .filter(|value| value.is_finite())
            .map(f64::abs)
            .fold(0.0_f64, f64::max);
        // Select the largest conventional unit that leaves at least one whole unit.
        // Bytes use binary prefixes so the scaling is unambiguous.
        let units: &[(f64, &str)] = match kind.as_unit() {
            "ns" => &[(1e9, "s"), (1e6, "ms"), (1e3, "us"), (1.0, "ns")],
            "bytes" => &[
                (1_099_511_627_776.0, "TiB"),
                (1_073_741_824.0, "GiB"),
                (1_048_576.0, "MiB"),
                (1024.0, "KiB"),
                (1.0, "bytes"),
            ],
            unit => return Self { unit, divisor: 1.0 },
        };
        let (divisor, unit) = units
            .iter()
            .copied()
            .find(|(divisor, _)| magnitude >= *divisor)
            .unwrap_or((1.0, kind.as_unit()));
        Self { unit, divisor }
    }

    pub(crate) fn value(self, value: f64) -> f64 {
        value / self.divisor
    }

    pub(crate) fn quantity(self, value: f64) -> String {
        format!("{} {}", format_value(self.value(value)), self.unit)
    }

    pub(crate) fn signed(self, value: f64) -> String {
        let sign = if value.is_sign_negative() { "-" } else { "+" };
        format!("{sign}{}", self.quantity(value.abs()))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn units_follow_metric_kinds_and_conventional_boundaries() {
        for kind in MetricKind::ALL {
            let expected = match kind.as_unit() {
                "ns" => "1.024 us",
                "bytes" => "1 KiB",
                _ => "1024 count",
            };
            assert_eq!(format_quantity(kind, 1024.0), expected);
        }
        for (value, expected) in [
            (999.0, "999 ns"),
            (1000.0, "1 us"),
            (1_000_000.0, "1 ms"),
            (1_000_000_000.0, "1 s"),
            (0.125, "0.125 ns"),
            (0.0, "0 ns"),
        ] {
            assert_eq!(format_quantity(MetricKind::WallTime, value), expected);
        }
        assert_eq!(
            format_quantity(MetricKind::AllocatedBytes, 1_048_576.0),
            "1 MiB"
        );
    }

    #[test]
    fn comparison_uses_one_scale_including_small_signed_deltas_and_gaps() {
        let scale = QuantityScale::new(MetricKind::WallTime, [900.0, 1100.0, f64::NAN]);
        assert_eq!(scale.quantity(900.0), "0.9 us");
        assert_eq!(scale.quantity(1100.0), "1.1 us");
        assert_eq!(scale.signed(-200.0), "-0.2 us");
        assert!(scale.value(f64::NAN).is_nan());
    }
}
