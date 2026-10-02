//! In-memory report rendering, scaled by finding count with fixed short charts.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;

use cbh_render::testing::{
    HIGH_FINDINGS, LOW_FINDINGS, NO_FINDINGS, ReportFixture, assert_full_report, assert_summary,
};
use cbh_render::{DEFAULT_SUMMARY_LIMIT, ReportFormat, render, render_markdown_summary};
use criterion::{Criterion, criterion_group, criterion_main};

fn full_reports(c: &mut Criterion) {
    for (name, format) in [
        ("text", ReportFormat::Text),
        ("markdown", ReportFormat::Markdown),
        ("json", ReportFormat::Json),
    ] {
        let mut group = c.benchmark_group(format!("cbh_render_reports/{name}"));
        for count in [LOW_FINDINGS, HIGH_FINDINGS] {
            let fixture = ReportFixture::new(count);
            let summaries = fixture.summaries();
            let input = fixture.input(&summaries);
            assert_full_report(&input, format);
            group.bench_function(format!("{count}_findings"), |b| {
                b.iter(|| black_box(render(black_box(&input), format, false)));
            });
        }
        if format == ReportFormat::Text {
            // History's quiet early return avoids all partition and chart rendering.
            // One representative format suffices; do not multiply empty-report variants.
            let fixture = ReportFixture::new(NO_FINDINGS);
            let summaries = fixture.summaries();
            let input = fixture.input(&summaries);
            assert_full_report(&input, format);
            group.bench_function("no_findings", |b| {
                b.iter(|| black_box(render(black_box(&input), format, false)));
            });
        }
        group.finish();
    }
}

fn markdown_summary(c: &mut Criterion) {
    assert!(LOW_FINDINGS <= DEFAULT_SUMMARY_LIMIT.get());
    assert!(HIGH_FINDINGS > DEFAULT_SUMMARY_LIMIT.get());
    let mut group = c.benchmark_group("cbh_render_reports/markdown_summary");
    for (count, shape) in [(LOW_FINDINGS, "retained"), (HIGH_FINDINGS, "truncated")] {
        let fixture = ReportFixture::new(count);
        let summaries = fixture.summaries();
        let input = fixture.input(&summaries);
        assert_summary(&input);
        group.bench_function(format!("{count}_findings_{shape}"), |b| {
            b.iter(|| {
                black_box(render_markdown_summary(
                    black_box(&input),
                    DEFAULT_SUMMARY_LIMIT,
                ))
            });
        });
    }
    group.finish();
}

criterion_group!(benches, full_reports, markdown_summary);
criterion_main!(benches);

::testing::set_allocator!();
