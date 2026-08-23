//! Callgrind benchmarks for human-readable nm histogram rendering.
//!
//! Paired with `nm_rendering.rs`, which covers the same low- and high-cardinality histograms
//! under wall-clock measurement. Rendering writes to a counting sink so output allocation does
//! not obscure formatter work.

#![allow(missing_docs, reason = "Benchmark code does not expose a public API.")]
#![cfg_attr(
    target_os = "linux",
    expect(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        unused_qualifications,
        reason = "These lints originate in Gungraun macro expansion and cannot be addressed in \
                  this benchmark."
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {
    // Gungraun requires Valgrind, which is available only on Linux.
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig, main};
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "linux")]
main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .args(["--branch-sim=yes", "--collect-bus=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    ),
    library_benchmark_groups = histogram
);

#[cfg(target_os = "linux")]
mod linux {
    use std::fmt::{self, Write};
    use std::hint::black_box;

    use gungraun::prelude::*;
    use nm::{Histogram, Magnitude};

    #[library_benchmark]
    #[bench::default(make_histogram(LOW_CARDINALITY_BUCKET_BOUNDS))]
    fn histogram_low(histogram: Histogram) -> (Histogram, usize) {
        let bytes = render(black_box(&histogram));
        (histogram, black_box(bytes))
    }

    #[library_benchmark]
    #[bench::default(make_histogram(HIGH_CARDINALITY_BUCKET_BOUNDS))]
    fn histogram_high(histogram: Histogram) -> (Histogram, usize) {
        let bytes = render(black_box(&histogram));
        (histogram, black_box(bytes))
    }

    library_benchmark_group!(
        name = histogram,
        benchmarks = [histogram_low, histogram_high]
    );

    fn make_histogram(bucket_bounds: &'static [Magnitude]) -> Histogram {
        Histogram::fake(
            bucket_bounds,
            vec![OBSERVATIONS_PER_BUCKET; bucket_bounds.len()],
            OBSERVATIONS_PER_BUCKET,
        )
    }

    fn render(histogram: &Histogram) -> usize {
        let mut output = ByteCounter::default();
        write!(&mut output, "{histogram}").expect("the counting sink accepts all formatted output");
        output.bytes
    }

    /// Counts formatted bytes without allocating output storage.
    #[derive(Debug, Default)]
    struct ByteCounter {
        bytes: usize,
    }

    impl Write for ByteCounter {
        fn write_str(&mut self, value: &str) -> fmt::Result {
            self.bytes = self
                .bytes
                .checked_add(value.len())
                .expect("formatted histogram output fits in usize");
            Ok(())
        }
    }

    /// Represents a compact histogram used for ordinary event reporting.
    const LOW_CARDINALITY_BUCKET_BOUNDS: &[Magnitude] = &[1, 10, 50, 100, 500, 1_000, 5_000];

    /// Exposes how rendering scales at the upper end of configured bucket cardinality.
    const HIGH_CARDINALITY_BUCKET_BOUNDS: &[Magnitude] = &[
        1,
        2,
        4,
        8,
        16,
        32,
        64,
        128,
        256,
        512,
        1_024,
        2_048,
        4_096,
        8_192,
        16_384,
        32_768,
        65_536,
        131_072,
        262_144,
        524_288,
        1_048_576,
        2_097_152,
        4_194_304,
        8_388_608,
        16_777_216,
        33_554_432,
        67_108_864,
        134_217_728,
        268_435_456,
        536_870_912,
        1_073_741_824,
    ];

    /// Produces a full-width bar for every bucket without approaching arithmetic limits.
    const OBSERVATIONS_PER_BUCKET: u64 = 100;
}
