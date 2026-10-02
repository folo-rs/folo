//! Callgrind report rendering, paired with `cbh_render_reports.rs`.
//!
//! The Markdown summary is Criterion-only because its production `HashSet` has randomized
//! hashing. Full reports measure formatting, charting and JSON projection, including their
//! allocator instructions, rather than modeling operating-system allocation latency.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]
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
    // Gungraun requires Valgrind, which is Linux-only.
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
    library_benchmark_groups = [text, markdown, json]
);

::testing::set_allocator!();

#[cfg(target_os = "linux")]
mod linux {
    use std::hint::black_box;

    use cbh_render::testing::{
        HIGH_FINDINGS, LOW_FINDINGS, NO_FINDINGS, ReportFixture, assert_full_report,
    };
    use cbh_render::{ReportFormat, ReportInput, render};
    use gungraun::{library_benchmark, library_benchmark_group};

    fn input(count: usize, format: ReportFormat) -> &'static ReportInput<'static> {
        // Each Callgrind case runs in its own process. Retaining this bounded fixture until
        // process exit keeps borrowing metadata construction and teardown outside measurement.
        let fixture = Box::leak(Box::new(ReportFixture::new(count)));
        let summaries = Box::leak(fixture.summaries().into_boxed_slice());
        let input = Box::leak(Box::new(fixture.input(summaries)));
        assert_full_report(input, format);
        input
    }

    #[library_benchmark]
    #[bench::report(input(LOW_FINDINGS, ReportFormat::Text))]
    fn text_2_findings(input: &ReportInput<'_>) -> String {
        black_box(render(black_box(input), ReportFormat::Text, false))
    }

    #[library_benchmark]
    #[bench::report(input(HIGH_FINDINGS, ReportFormat::Text))]
    fn text_20_findings(input: &ReportInput<'_>) -> String {
        black_box(render(black_box(input), ReportFormat::Text, false))
    }

    #[library_benchmark]
    #[bench::report(input(NO_FINDINGS, ReportFormat::Text))]
    fn text_no_findings(input: &ReportInput<'_>) -> String {
        black_box(render(black_box(input), ReportFormat::Text, false))
    }

    #[library_benchmark]
    #[bench::report(input(LOW_FINDINGS, ReportFormat::Markdown))]
    fn markdown_2_findings(input: &ReportInput<'_>) -> String {
        black_box(render(black_box(input), ReportFormat::Markdown, false))
    }

    #[library_benchmark]
    #[bench::report(input(HIGH_FINDINGS, ReportFormat::Markdown))]
    fn markdown_20_findings(input: &ReportInput<'_>) -> String {
        black_box(render(black_box(input), ReportFormat::Markdown, false))
    }

    #[library_benchmark]
    #[bench::report(input(LOW_FINDINGS, ReportFormat::Json))]
    fn json_2_findings(input: &ReportInput<'_>) -> String {
        black_box(render(black_box(input), ReportFormat::Json, false))
    }

    #[library_benchmark]
    #[bench::report(input(HIGH_FINDINGS, ReportFormat::Json))]
    fn json_20_findings(input: &ReportInput<'_>) -> String {
        black_box(render(black_box(input), ReportFormat::Json, false))
    }

    library_benchmark_group!(
        name = text,
        benchmarks = [text_2_findings, text_20_findings, text_no_findings]
    );
    library_benchmark_group!(
        name = markdown,
        benchmarks = [markdown_2_findings, markdown_20_findings]
    );
    library_benchmark_group!(
        name = json,
        benchmarks = [json_2_findings, json_20_findings]
    );
}
